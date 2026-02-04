use {
    rayon::prelude::*,
    solana_sdk::{
        compute_budget::ComputeBudgetInstruction,
        hash::Hash,
        message::Message,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
        system_instruction,
        transaction::Transaction,
    },
};

// Compute unit settings for transfer transactions
const TRANSFER_TRANSACTION_COMPUTE_UNIT: u32 = 600;
const TRANSFER_TRANSACTION_LOADED_ACCOUNTS_DATA_SIZE: u32 = 30 * 1024;

/// Generator for benchmark transactions using disjoint keypair pools.
///
/// Creates simple transfer transactions between keypairs, using separate
/// source and destination pools to eliminate account lock contention:
/// - Source keypairs (first half) are only used as signers/fee payers
/// - Destination pubkeys (second half) are only used as transfer recipients
/// - This ensures no circular dependencies where TX(n)'s destination is TX(n+1)'s source
///
/// With N total keypairs:
/// - N/2 source keypairs, N/2 destination pubkeys
/// - Transactions can be processed in parallel since sources never overlap with destinations
/// - Generates N/2 * N/2 unique source-destination pairs before repeating
pub struct TransactionGenerator {
    /// Source keypairs - used for signing transactions (first half of input keypairs)
    source_keypairs: Vec<Keypair>,
    /// Destination pubkeys - transfer recipients (second half of input keypairs)
    dest_pubkeys: Vec<Pubkey>,
    /// Current source keypair index
    current_source_index: usize,
    /// Destination offset - rotates after each full cycle through sources
    /// This creates num_sources * num_dests unique pairs before repeating
    dest_offset: usize,
    compute_unit_price: Option<u64>,
    skip_data_size_limit: bool,
    /// Counter tracking number of transactions generated
    nonce: u64,
}

impl TransactionGenerator {
    /// Create a new transaction generator with disjoint source/destination pools.
    ///
    /// # Arguments
    /// * `keypairs` - Funded keypairs to use for transactions (must have at least 4).
    ///   The first half will be used as sources (signers), the second half as destinations.
    /// * `compute_unit_price` - Optional priority fee in micro-lamports per compute unit
    /// * `skip_data_size_limit` - Whether to skip setting the loaded accounts data size limit
    ///
    /// # Panics
    /// Panics if fewer than 4 keypairs are provided (need at least 2 sources and 2 destinations).
    pub fn new(
        keypairs: Vec<Keypair>,
        compute_unit_price: Option<u64>,
        skip_data_size_limit: bool,
    ) -> Self {
        assert!(
            keypairs.len() >= 4,
            "Need at least 4 keypairs (2 sources + 2 destinations)"
        );

        let half = keypairs.len() / 2;

        // Split into disjoint pools: first half = sources, second half = destinations
        let mut keypair_iter = keypairs.into_iter();
        let source_keypairs: Vec<Keypair> = keypair_iter.by_ref().take(half).collect();
        let dest_pubkeys: Vec<Pubkey> = keypair_iter.map(|kp| kp.pubkey()).collect();

        Self {
            source_keypairs,
            dest_pubkeys,
            current_source_index: 0,
            dest_offset: 0,
            compute_unit_price,
            skip_data_size_limit,
            nonce: 0,
        }
    }

    /// Generate the next transaction with the given blockhash.
    ///
    /// Uses disjoint source/destination pools:
    /// - Cycles through source keypairs in round-robin fashion
    /// - Destination is computed as (source_index + dest_offset) % num_dests
    /// - After each full cycle through sources, dest_offset increments
    /// - This creates num_sources * num_dests unique pairs before repeating
    /// - Since sources and destinations never overlap, consecutive transactions
    ///   don't create account lock dependencies
    pub fn next_transaction(&mut self, blockhash: &Hash) -> Transaction {
        let from_keypair = &self.source_keypairs[self.current_source_index];
        // Compute destination index using offset pattern for maximum unique pairs
        let dest_index = (self.current_source_index + self.dest_offset) % self.dest_pubkeys.len();
        let to_pubkey = self.dest_pubkeys[dest_index];

        // Fixed 1 lamport transfer
        let transfer_amount = 1;

        // Build instructions
        let mut instructions = vec![];

        // Add loaded accounts data size limit if not skipped
        if !self.skip_data_size_limit {
            instructions.push(
                ComputeBudgetInstruction::set_loaded_accounts_data_size_limit(
                    TRANSFER_TRANSACTION_LOADED_ACCOUNTS_DATA_SIZE,
                ),
            );
        }

        // Add transfer instruction
        instructions.push(system_instruction::transfer(
            &from_keypair.pubkey(),
            &to_pubkey,
            transfer_amount,
        ));

        // Add compute unit price if specified
        if let Some(price) = self.compute_unit_price {
            instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(
                TRANSFER_TRANSACTION_COMPUTE_UNIT,
            ));
            instructions.push(ComputeBudgetInstruction::set_compute_unit_price(price));
        }

        let message = Message::new(&instructions, Some(&from_keypair.pubkey()));
        let tx = Transaction::new(&[from_keypair], message, *blockhash);

        // Advance source index
        self.current_source_index = (self.current_source_index + 1) % self.source_keypairs.len();

        // After completing a full cycle through sources, rotate destination offset
        if self.current_source_index == 0 {
            self.dest_offset = (self.dest_offset + 1) % self.dest_pubkeys.len();
        }

        self.nonce = self.nonce.wrapping_add(1);

        tx
    }

    /// Get the number of source keypairs available.
    pub fn source_count(&self) -> usize {
        self.source_keypairs.len()
    }

    /// Get the number of destination pubkeys available.
    pub fn dest_count(&self) -> usize {
        self.dest_pubkeys.len()
    }

    /// Get the total number of keypairs (sources + destinations).
    pub fn keypair_count(&self) -> usize {
        self.source_keypairs.len() + self.dest_pubkeys.len()
    }

    /// Get the current nonce value (number of transactions generated).
    pub fn nonce(&self) -> u64 {
        self.nonce
    }

    /// Reset the generator to start from the first keypair.
    pub fn reset(&mut self) {
        self.current_source_index = 0;
        self.dest_offset = 0;
        // Note: nonce continues incrementing for transaction counting purposes
    }

    /// Generate a batch of transactions with parallel signing.
    ///
    /// This pre-computes source/destination indices for the batch, then uses
    /// Rayon to parallelize transaction creation and signing across CPU cores.
    ///
    /// Returns a vector of signed transactions ready to send.
    pub fn generate_batch(&mut self, blockhash: &Hash, batch_size: usize) -> Vec<Transaction> {
        let num_sources = self.source_keypairs.len();
        let num_dests = self.dest_pubkeys.len();

        // Pre-compute (source_index, dest_index) pairs for the batch
        let mut indices: Vec<(usize, usize)> = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            let dest_index = (self.current_source_index + self.dest_offset) % num_dests;
            indices.push((self.current_source_index, dest_index));

            // Advance source index
            self.current_source_index = (self.current_source_index + 1) % num_sources;

            // After completing a full cycle through sources, rotate destination offset
            if self.current_source_index == 0 {
                self.dest_offset = (self.dest_offset + 1) % num_dests;
            }
        }

        self.nonce = self.nonce.wrapping_add(batch_size as u64);

        // Generate transactions in parallel using Rayon
        let source_keypairs = &self.source_keypairs;
        let dest_pubkeys = &self.dest_pubkeys;
        let compute_unit_price = self.compute_unit_price;
        let skip_data_size_limit = self.skip_data_size_limit;

        indices
            .into_par_iter()
            .map(|(source_index, dest_index)| {
                let from_keypair = &source_keypairs[source_index];
                let to_pubkey = dest_pubkeys[dest_index];

                // Fixed 1 lamport transfer
                let transfer_amount = 1;

                // Build instructions
                let mut instructions = vec![];

                if !skip_data_size_limit {
                    instructions.push(
                        ComputeBudgetInstruction::set_loaded_accounts_data_size_limit(
                            TRANSFER_TRANSACTION_LOADED_ACCOUNTS_DATA_SIZE,
                        ),
                    );
                }

                instructions.push(system_instruction::transfer(
                    &from_keypair.pubkey(),
                    &to_pubkey,
                    transfer_amount,
                ));

                if let Some(price) = compute_unit_price {
                    instructions.push(ComputeBudgetInstruction::set_compute_unit_limit(
                        TRANSFER_TRANSACTION_COMPUTE_UNIT,
                    ));
                    instructions.push(ComputeBudgetInstruction::set_compute_unit_price(price));
                }

                let message = Message::new(&instructions, Some(&from_keypair.pubkey()));
                Transaction::new(&[from_keypair], message, *blockhash)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_transaction_generator_creation() {
        let keypairs: Vec<Keypair> = (0..8).map(|_| Keypair::new()).collect();
        let generator = TransactionGenerator::new(keypairs, None, false);
        // 8 keypairs split into 4 sources + 4 destinations
        assert_eq!(generator.source_count(), 4);
        assert_eq!(generator.dest_count(), 4);
        assert_eq!(generator.keypair_count(), 8);
    }

    #[test]
    #[should_panic(expected = "Need at least 4 keypairs")]
    fn test_transaction_generator_requires_four_keypairs() {
        let keypairs: Vec<Keypair> = (0..2).map(|_| Keypair::new()).collect();
        TransactionGenerator::new(keypairs, None, false);
    }

    #[test]
    fn test_disjoint_pools() {
        // Verify that source and destination pools don't overlap
        let keypairs: Vec<Keypair> = (0..10).map(|_| Keypair::new()).collect();
        let pubkeys: Vec<Pubkey> = keypairs.iter().map(|kp| kp.pubkey()).collect();

        let generator = TransactionGenerator::new(keypairs, None, false);

        // Get all source pubkeys
        let source_pubkeys: HashSet<Pubkey> = generator
            .source_keypairs
            .iter()
            .map(|kp| kp.pubkey())
            .collect();

        // Get all dest pubkeys
        let dest_pubkeys: HashSet<Pubkey> = generator.dest_pubkeys.iter().copied().collect();

        // Verify no overlap
        assert!(
            source_pubkeys.is_disjoint(&dest_pubkeys),
            "Source and destination pools must be disjoint"
        );

        // Verify sources come from first half
        for (i, pubkey) in source_pubkeys.iter().enumerate() {
            assert!(
                pubkeys[..5].contains(pubkey),
                "Source {} should be from first half",
                i
            );
        }

        // Verify destinations come from second half
        for (i, pubkey) in dest_pubkeys.iter().enumerate() {
            assert!(
                pubkeys[5..].contains(pubkey),
                "Destination {} should be from second half",
                i
            );
        }
    }

    #[test]
    fn test_no_circular_dependencies() {
        // The key test: verify that consecutive transactions don't share accounts
        let keypairs: Vec<Keypair> = (0..20).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate several transactions and check for conflicts
        let transactions: Vec<Transaction> = (0..10)
            .map(|_| generator.next_transaction(&blockhash))
            .collect();

        // Check each consecutive pair
        for i in 0..transactions.len() - 1 {
            let tx1 = &transactions[i];
            let tx2 = &transactions[i + 1];

            // Get writable accounts from each transaction
            // For a transfer: account_keys[0] = source (writable), account_keys[1] = dest (writable)
            let tx1_source = tx1.message.account_keys[0];
            let tx1_dest = tx1.message.account_keys[1];
            let tx2_source = tx2.message.account_keys[0];
            let tx2_dest = tx2.message.account_keys[1];

            // The critical check: tx1's destination should NOT be tx2's source
            // This was the circular dependency in the old implementation
            assert_ne!(
                tx1_dest, tx2_source,
                "TX{}'s destination should not be TX{}'s source (circular dep)",
                i,
                i + 1
            );

            // Also verify no other overlaps between writable accounts
            assert_ne!(
                tx1_source, tx2_source,
                "TX{} and TX{} should have different sources",
                i,
                i + 1
            );
            assert_ne!(
                tx1_source, tx2_dest,
                "TX{}'s source should not be TX{}'s destination",
                i,
                i + 1
            );
            assert_ne!(
                tx1_dest, tx2_dest,
                "TX{} and TX{} should have different destinations",
                i,
                i + 1
            );
        }
    }

    #[test]
    fn test_transaction_generator_round_robin() {
        let keypairs: Vec<Keypair> = (0..8).map(|_| Keypair::new()).collect();
        let source_pubkeys: Vec<Pubkey> = keypairs[..4].iter().map(|kp| kp.pubkey()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate transactions and verify they cycle through source keypairs
        let tx1 = generator.next_transaction(&blockhash);
        assert_eq!(tx1.message.account_keys[0], source_pubkeys[0]);

        let tx2 = generator.next_transaction(&blockhash);
        assert_eq!(tx2.message.account_keys[0], source_pubkeys[1]);

        let tx3 = generator.next_transaction(&blockhash);
        assert_eq!(tx3.message.account_keys[0], source_pubkeys[2]);

        let tx4 = generator.next_transaction(&blockhash);
        assert_eq!(tx4.message.account_keys[0], source_pubkeys[3]);

        // Should wrap around to first source
        let tx5 = generator.next_transaction(&blockhash);
        assert_eq!(tx5.message.account_keys[0], source_pubkeys[0]);
    }

    #[test]
    fn test_transaction_generator_unique_transactions() {
        let keypairs: Vec<Keypair> = (0..20).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate transactions - all should be unique
        let transactions: Vec<_> = (0..20)
            .map(|_| generator.next_transaction(&blockhash))
            .collect();

        // All transactions should have unique signatures
        let signatures: HashSet<_> = transactions.iter().map(|tx| tx.signatures[0]).collect();
        assert_eq!(
            signatures.len(),
            20,
            "All 20 transactions should have unique signatures"
        );
    }

    #[test]
    fn test_transaction_generator_with_priority_fee() {
        let keypairs: Vec<Keypair> = (0..4).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, Some(1000), false);
        let blockhash = Hash::new_unique();

        let tx = generator.next_transaction(&blockhash);

        // Should have 4 instructions: data size limit, transfer, compute limit, compute price
        assert_eq!(tx.message.instructions.len(), 4);
    }

    #[test]
    fn test_transaction_generator_skip_data_size() {
        let keypairs: Vec<Keypair> = (0..4).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, true);
        let blockhash = Hash::new_unique();

        let tx = generator.next_transaction(&blockhash);

        // Should only have 1 instruction: transfer
        assert_eq!(tx.message.instructions.len(), 1);
    }

    #[test]
    fn test_transaction_generator_reset() {
        let keypairs: Vec<Keypair> = (0..8).map(|_| Keypair::new()).collect();
        let first_source_pubkey = keypairs[0].pubkey();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Advance a few times
        generator.next_transaction(&blockhash);
        generator.next_transaction(&blockhash);
        let nonce_before_reset = generator.nonce();

        // Reset
        generator.reset();

        // Should start from first source keypair again
        let tx = generator.next_transaction(&blockhash);
        assert_eq!(tx.message.account_keys[0], first_source_pubkey);

        // But nonce should have continued (for uniqueness)
        assert_eq!(generator.nonce(), nonce_before_reset + 1);
    }

    #[test]
    fn test_generate_batch() {
        let keypairs: Vec<Keypair> = (0..20).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate a batch of 30 transactions
        let batch = generator.generate_batch(&blockhash, 30);

        assert_eq!(batch.len(), 30);

        // All transactions should be unique
        let signatures: HashSet<_> = batch.iter().map(|tx| tx.signatures[0]).collect();
        assert_eq!(signatures.len(), 30, "All batch transactions should be unique");

        // Nonce should have advanced by batch size
        assert_eq!(generator.nonce(), 30);
    }

    #[test]
    fn test_generate_batch_no_circular_deps() {
        // Verify batch generation produces non-conflicting transactions
        // when batch_size <= min(num_sources, num_dests)
        let keypairs: Vec<Keypair> = (0..100).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // With 100 keypairs = 50 sources + 50 dests, batch of 50 should have zero conflicts
        let batch = generator.generate_batch(&blockhash, 50);

        // Check ALL types of conflicts
        for i in 0..batch.len() {
            for j in (i + 1)..batch.len() {
                let tx1 = &batch[i];
                let tx2 = &batch[j];

                let tx1_source = tx1.message.account_keys[0];
                let tx1_dest = tx1.message.account_keys[1];
                let tx2_source = tx2.message.account_keys[0];
                let tx2_dest = tx2.message.account_keys[1];

                // Check for source conflicts (both transactions write to same source)
                assert_ne!(
                    tx1_source, tx2_source,
                    "TX{} and TX{} conflict on source {}",
                    i, j, tx1_source
                );

                // Check for destination conflicts (both transactions write to same dest)
                assert_ne!(
                    tx1_dest, tx2_dest,
                    "TX{} and TX{} conflict on destination {}",
                    i, j, tx1_dest
                );

                // Check for cross-pool contamination (shouldn't happen with disjoint pools)
                assert_ne!(
                    tx1_dest, tx2_source,
                    "TX{}'s dest should not be TX{}'s source",
                    i, j
                );
                assert_ne!(
                    tx2_dest, tx1_source,
                    "TX{}'s dest should not be TX{}'s source",
                    j, i
                );
            }
        }
    }

    #[test]
    fn test_batch_size_vs_keypairs_conflict_rate() {
        // Demonstrate that batch_size > num_sources causes source conflicts
        let keypairs: Vec<Keypair> = (0..20).map(|_| Keypair::new()).collect();
        // 20 keypairs = 10 sources + 10 dests
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate 30 transactions with only 10 sources - WILL have conflicts
        let batch = generator.generate_batch(&blockhash, 30);

        // Count source conflicts
        let mut source_conflicts = 0;
        for i in 0..batch.len() {
            for j in (i + 1)..batch.len() {
                if batch[i].message.account_keys[0] == batch[j].message.account_keys[0] {
                    source_conflicts += 1;
                }
            }
        }

        // With 30 txs and 10 sources, each source is used 3 times
        // Conflicts per source: C(3,2) = 3 pairs
        // Total conflicts: 10 sources * 3 = 30
        assert!(
            source_conflicts > 0,
            "Expected source conflicts when batch_size > num_sources"
        );
        println!(
            "With 30 txs and 10 sources: {} source conflict pairs",
            source_conflicts
        );
    }

    #[test]
    fn test_generate_batch_matches_sequential() {
        // Verify that batch generation produces the same transactions as sequential generation
        let keypairs1: Vec<Keypair> = (0..10).map(|_| Keypair::new()).collect();
        let keypairs2: Vec<Keypair> = keypairs1
            .iter()
            .map(|kp| Keypair::from_bytes(&kp.to_bytes()).unwrap())
            .collect();

        let mut gen1 = TransactionGenerator::new(keypairs1, None, false);
        let mut gen2 = TransactionGenerator::new(keypairs2, None, false);
        let blockhash = Hash::new_unique();

        // Generate 10 transactions sequentially
        let sequential: Vec<_> = (0..10)
            .map(|_| gen1.next_transaction(&blockhash))
            .collect();

        // Generate 10 transactions in batch
        let batch = gen2.generate_batch(&blockhash, 10);

        // They should have the same signatures (same source-dest pairs, same blockhash)
        for i in 0..10 {
            assert_eq!(
                sequential[i].signatures[0], batch[i].signatures[0],
                "Transaction {} should match between sequential and batch",
                i
            );
        }
    }
}
