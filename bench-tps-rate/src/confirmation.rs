use {
    log::{info, warn},
    solana_sdk::commitment_config::CommitmentConfig,
    solana_tps_client::TpsClient,
    std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread::{self, JoinHandle},
        time::Duration,
    },
};

/// Result from the confirmation tracker.
pub struct ConfirmationResult {
    pub confirmed: usize,
}

/// Configuration for the confirmation tracker.
pub struct ConfirmationTrackerConfig {
    /// How often to poll for transaction count (in milliseconds).
    pub poll_interval_ms: u64,
}

impl Default for ConfirmationTrackerConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 400,
        }
    }
}

/// Spawns a confirmation tracker thread that monitors transaction confirmations
/// using transaction count polling (same approach as bench-tps).
///
/// The tracker polls `get_transaction_count` and uses the delta during the
/// measurement window to determine how many transactions were confirmed.
///
/// Confirmations are only counted between when `measurement_start_signal` is set
/// and when `measurement_end_signal` is set.
///
/// The tracker stops when `measurement_end_signal` is set.
pub fn spawn_confirmation_tracker(
    client: Arc<dyn TpsClient + Send + Sync>,
    measurement_start_signal: Arc<AtomicBool>,
    measurement_end_signal: Arc<AtomicBool>,
    config: ConfirmationTrackerConfig,
) -> JoinHandle<ConfirmationResult> {
    thread::Builder::new()
        .name("confirmation-tracker".to_string())
        .spawn(move || {
            confirmation_tracker_loop(
                client,
                measurement_start_signal,
                measurement_end_signal,
                config,
            )
        })
        .expect("Failed to spawn confirmation tracker thread")
}

fn confirmation_tracker_loop(
    client: Arc<dyn TpsClient + Send + Sync>,
    measurement_start_signal: Arc<AtomicBool>,
    measurement_end_signal: Arc<AtomicBool>,
    config: ConfirmationTrackerConfig,
) -> ConfirmationResult {
    // Use the same commitment as sample_txs in bench-tps
    let tx_count_commitment = CommitmentConfig::processed();

    let poll_interval = Duration::from_millis(config.poll_interval_ms);

    // Track tx count at measurement start and current
    let mut measurement_started = false;
    let mut start_tx_count: u64 = 0;
    let mut last_tx_count: u64 = 0;

    info!(
        "Confirmation tracker started (tx-count based), using get_transaction_count_with_commitment({:?})",
        tx_count_commitment.commitment
    );

    loop {
        if measurement_end_signal.load(Ordering::Relaxed) {
            let confirmed = last_tx_count.saturating_sub(start_tx_count);
            info!(
                "Measurement ended. tx_count: {} -> {} (confirmed: {})",
                start_tx_count, last_tx_count, confirmed
            );
            return ConfirmationResult {
                confirmed: confirmed as usize,
            };
        }

        let measurement_active = measurement_start_signal.load(Ordering::Relaxed);

        // On transition into measurement, snapshot the starting tx count
        if measurement_active && !measurement_started {
            match client.get_transaction_count_with_commitment(tx_count_commitment) {
                Ok(tx_count) => {
                    measurement_started = true;
                    start_tx_count = tx_count;
                    last_tx_count = tx_count;
                    info!("Measurement window started. start_tx_count={}", start_tx_count);
                }
                Err(e) => {
                    warn!(
                        "Couldn't get initial transaction count to start measurement: {:?}",
                        e
                    );
                }
            }
        }

        // Poll tx count periodically during measurement
        if measurement_started {
            match client.get_transaction_count_with_commitment(tx_count_commitment) {
                Ok(tx_count) => {
                    // Monotonic clamp (same as sample_txs)
                    if tx_count >= last_tx_count {
                        last_tx_count = tx_count;
                    } else {
                        info!("Expected tx_count({}) >= last_tx_count({})", tx_count, last_tx_count);
                    }
                }
                Err(e) => {
                    warn!("Couldn't get transaction count: {:?}", e);
                }
            }
        }

        thread::sleep(poll_interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confirmation_tracker_config_default() {
        let config = ConfirmationTrackerConfig::default();
        assert_eq!(config.poll_interval_ms, 400);
    }
}
