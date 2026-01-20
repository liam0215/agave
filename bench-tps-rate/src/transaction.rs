use {
    rayon::prelude::*,
    solana_sdk::{
        compute_budget::ComputeBudgetInstruction,
        hash::Hash,
        message::Message,
        signature::{Keypair, Signer},
        system_instruction,
        transaction::Transaction,
    },
};

// Compute unit settings for transfer transactions
const TRANSFER_TRANSACTION_COMPUTE_UNIT: u32 = 600;
const TRANSFER_TRANSACTION_LOADED_ACCOUNTS_DATA_SIZE: u32 = 30 * 1024;

/// Generator for benchmark transactions.
///
/// Creates simple transfer transactions between keypairs.
/// Uses destination rotation (like bench-tps) to ensure uniqueness:
/// - After cycling through all source keypairs, rotates the destination offset
/// - This creates N*(N-1) unique source-destination pairs before repeating
/// - Uses fixed 1 lamport transfers to minimize fund consumption
pub struct TransactionGenerator {
    keypairs: Vec<Keypair>,
    /// Current source keypair index
    current_index: usize,
    /// Destination offset - rotates after each full cycle through sources
    dest_offset: usize,
    compute_unit_price: Option<u64>,
    skip_data_size_limit: bool,
    /// Counter tracking number of transactions generated
    nonce: u64,
}

impl TransactionGenerator {
    /// Create a new transaction generator.
    ///
    /// # Arguments
    /// * `keypairs` - Funded keypairs to use for transactions (must have at least 2)
    /// * `compute_unit_price` - Optional priority fee in micro-lamports per compute unit
    /// * `skip_data_size_limit` - Whether to skip setting the loaded accounts data size limit
    pub fn new(
        keypairs: Vec<Keypair>,
        compute_unit_price: Option<u64>,
        skip_data_size_limit: bool,
    ) -> Self {
        assert!(keypairs.len() >= 2, "Need at least 2 keypairs");

        Self {
            keypairs,
            current_index: 0,
            dest_offset: 1, // Start with offset 1 (next keypair)
            compute_unit_price,
            skip_data_size_limit,
            nonce: 0,
        }
    }

    /// Generate the next transaction with the given blockhash.
    ///
    /// Uses destination rotation (similar to bench-tps) for uniqueness:
    /// - Cycles through source keypairs in round-robin fashion
    /// - After completing a full cycle, rotates the destination offset
    /// - This creates N*(N-1) unique source-destination pairs before any repeat
    /// - Uses fixed 1 lamport transfer amount (like bench-tps)
    pub fn next_transaction(&mut self, blockhash: &Hash) -> Transaction {
        let num_keypairs = self.keypairs.len();
        let from_index = self.current_index;
        // Destination is source + offset, wrapping around
        let to_index = (from_index + self.dest_offset) % num_keypairs;

        let from_keypair = &self.keypairs[from_index];
        let to_pubkey = self.keypairs[to_index].pubkey();

        // Fixed 1 lamport transfer (like bench-tps)
        // Uniqueness comes from destination rotation, not transfer amount
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

        // Advance to next source keypair
        self.current_index = (self.current_index + 1) % num_keypairs;

        // After cycling through all sources, rotate destination offset
        // This ensures we use all N*(N-1) unique source-destination pairs
        if self.current_index == 0 {
            self.dest_offset = (self.dest_offset % (num_keypairs - 1)) + 1;
        }

        self.nonce = self.nonce.wrapping_add(1);

        tx
    }

    /// Get the number of keypairs available.
    pub fn keypair_count(&self) -> usize {
        self.keypairs.len()
    }

    /// Get the current nonce value (number of transactions generated).
    pub fn nonce(&self) -> u64 {
        self.nonce
    }

    /// Reset the generator to start from the first keypair.
    pub fn reset(&mut self) {
        self.current_index = 0;
        self.dest_offset = 1;
        // Note: nonce continues incrementing for transaction counting purposes
    }

    /// Generate a batch of transactions with parallel signing.
    ///
    /// This pre-computes source/destination indices for the batch, then uses
    /// Rayon to parallelize transaction creation and signing across CPU cores.
    ///
    /// Returns a vector of signed transactions ready to send.
    pub fn generate_batch(&mut self, blockhash: &Hash, batch_size: usize) -> Vec<Transaction> {
        let num_keypairs = self.keypairs.len();

        // Pre-compute (from_index, to_index) pairs for the batch
        let mut indices: Vec<(usize, usize)> = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            let from_index = self.current_index;
            let to_index = (from_index + self.dest_offset) % num_keypairs;
            indices.push((from_index, to_index));

            // Advance state (same logic as next_transaction)
            self.current_index = (self.current_index + 1) % num_keypairs;
            if self.current_index == 0 {
                self.dest_offset = (self.dest_offset % (num_keypairs - 1)) + 1;
            }
        }

        self.nonce = self.nonce.wrapping_add(batch_size as u64);

        // Generate transactions in parallel using Rayon
        let keypairs = &self.keypairs;
        let compute_unit_price = self.compute_unit_price;
        let skip_data_size_limit = self.skip_data_size_limit;

        indices
            .into_par_iter()
            .map(|(from_index, to_index)| {
                let from_keypair = &keypairs[from_index];
                let to_pubkey = keypairs[to_index].pubkey();

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

    #[test]
    fn test_transaction_generator_creation() {
        let keypairs: Vec<Keypair> = (0..4).map(|_| Keypair::new()).collect();
        let generator = TransactionGenerator::new(keypairs, None, false);
        assert_eq!(generator.keypair_count(), 4);
    }

    #[test]
    #[should_panic(expected = "Need at least 2 keypairs")]
    fn test_transaction_generator_requires_two_keypairs() {
        let keypairs = vec![Keypair::new()];
        TransactionGenerator::new(keypairs, None, false);
    }

    #[test]
    fn test_transaction_generator_round_robin() {
        let keypairs: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
        let pubkeys: Vec<_> = keypairs.iter().map(|kp| kp.pubkey()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate transactions and verify they cycle through keypairs
        let tx1 = generator.next_transaction(&blockhash);
        assert_eq!(tx1.message.account_keys[0], pubkeys[0]);

        let tx2 = generator.next_transaction(&blockhash);
        assert_eq!(tx2.message.account_keys[0], pubkeys[1]);

        let tx3 = generator.next_transaction(&blockhash);
        assert_eq!(tx3.message.account_keys[0], pubkeys[2]);

        // Should wrap around
        let tx4 = generator.next_transaction(&blockhash);
        assert_eq!(tx4.message.account_keys[0], pubkeys[0]);
    }

    #[test]
    fn test_transaction_generator_unique_transactions_via_destination_rotation() {
        // Use enough keypairs so destination rotation provides uniqueness
        // With N keypairs, we get N*(N-1) unique source-destination pairs
        let keypairs: Vec<Keypair> = (0..10).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate 10 transactions (one full cycle through sources)
        // Each should have a unique signature due to different source-destination pairs
        let transactions: Vec<_> = (0..10)
            .map(|_| generator.next_transaction(&blockhash))
            .collect();

        // All transactions in the first cycle should be unique (different sources)
        for i in 0..10 {
            for j in (i + 1)..10 {
                assert_ne!(
                    transactions[i].signatures[0],
                    transactions[j].signatures[0],
                    "Transactions {} and {} should differ",
                    i,
                    j
                );
            }
        }

        // Generate another 10 transactions (second cycle, different destination offset)
        let transactions2: Vec<_> = (0..10)
            .map(|_| generator.next_transaction(&blockhash))
            .collect();

        // Transactions from second cycle should also be unique from first cycle
        // because destination offset rotated
        assert_ne!(
            transactions[0].signatures[0],
            transactions2[0].signatures[0],
            "Same source should have different destination after rotation"
        );
    }

    #[test]
    fn test_transaction_generator_with_priority_fee() {
        let keypairs: Vec<Keypair> = (0..2).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, Some(1000), false);
        let blockhash = Hash::new_unique();

        let tx = generator.next_transaction(&blockhash);

        // Should have 4 instructions: data size limit, transfer, compute limit, compute price
        assert_eq!(tx.message.instructions.len(), 4);
    }

    #[test]
    fn test_transaction_generator_skip_data_size() {
        let keypairs: Vec<Keypair> = (0..2).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, true);
        let blockhash = Hash::new_unique();

        let tx = generator.next_transaction(&blockhash);

        // Should only have 1 instruction: transfer
        assert_eq!(tx.message.instructions.len(), 1);
    }

    #[test]
    fn test_transaction_generator_reset() {
        let keypairs: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
        let first_pubkey = keypairs[0].pubkey();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Advance a few times
        generator.next_transaction(&blockhash);
        generator.next_transaction(&blockhash);
        let nonce_before_reset = generator.nonce();

        // Reset
        generator.reset();

        // Should start from first keypair again
        let tx = generator.next_transaction(&blockhash);
        assert_eq!(tx.message.account_keys[0], first_pubkey);

        // But nonce should have continued (for uniqueness)
        assert_eq!(generator.nonce(), nonce_before_reset + 1);
    }

    #[test]
    fn test_generate_batch() {
        let keypairs: Vec<Keypair> = (0..10).map(|_| Keypair::new()).collect();
        let mut generator = TransactionGenerator::new(keypairs, None, false);
        let blockhash = Hash::new_unique();

        // Generate a batch of 20 transactions
        let batch = generator.generate_batch(&blockhash, 20);

        assert_eq!(batch.len(), 20);

        // All transactions should be unique
        for i in 0..20 {
            for j in (i + 1)..20 {
                assert_ne!(
                    batch[i].signatures[0],
                    batch[j].signatures[0],
                    "Batch transactions {} and {} should differ",
                    i,
                    j
                );
            }
        }

        // Nonce should have advanced by batch size
        assert_eq!(generator.nonce(), 20);
    }

    #[test]
    fn test_generate_batch_matches_sequential() {
        // Verify that batch generation produces the same transactions as sequential generation
        let keypairs1: Vec<Keypair> = (0..5).map(|_| Keypair::new()).collect();
        let keypairs2: Vec<Keypair> = keypairs1.iter().map(|kp| {
            Keypair::from_bytes(&kp.to_bytes()).unwrap()
        }).collect();

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
                sequential[i].signatures[0],
                batch[i].signatures[0],
                "Transaction {} should match between sequential and batch",
                i
            );
        }
    }
}
