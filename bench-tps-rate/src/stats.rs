/// Statistics report for the benchmark.
///
/// Note: This uses a steady-state measurement model where:
/// - `sent` = transactions sent during the measurement window
/// - `confirmed` = confirmations observed during the measurement window
///
/// These are independent measurements. Confirmations include some warmup
/// transactions, while some measurement transactions will confirm after
/// the window ends. Under steady-state, these balance out.
#[derive(Debug, Clone)]
pub struct StatsReport {
    /// Transactions sent during the measurement window
    pub sent: usize,
    /// Confirmations observed during the measurement window
    pub confirmed: usize,
}

impl StatsReport {
    /// Create a new statistics report.
    pub fn new(sent: usize, confirmed: usize) -> Self {
        Self { sent, confirmed }
    }

    /// Print a formatted report to stdout.
    pub fn print(&self, duration_secs: f64, target_tps: u64) {
        let send_tps = if duration_secs > 0.0 {
            self.sent as f64 / duration_secs
        } else {
            0.0
        };

        let confirm_tps = if duration_secs > 0.0 {
            self.confirmed as f64 / duration_secs
        } else {
            0.0
        };

        println!();
        println!("============ Benchmark Results ============");
        println!("Duration:           {:.1}s", duration_secs);
        println!("Target Rate:        {} TPS", target_tps);
        println!();
        println!("Transactions (during measurement window):");
        println!("  Sent:             {} ({:.1} TPS)", self.sent, send_tps);
        println!("  Confirmed:        {} ({:.1} TPS)", self.confirmed, confirm_tps);
        println!("============================================");
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_report() {
        let report = StatsReport::new(100, 95);
        assert_eq!(report.sent, 100);
        assert_eq!(report.confirmed, 95);
    }
}
