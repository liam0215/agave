use std::time::{Duration, Instant};

/// Rate limiter for controlling transaction send rate.
///
/// Uses a uniform distribution with fixed interval = 1,000,000,000 ns / target_tps
/// to ensure precise pacing of transactions.
pub struct RateLimiter {
    target_tps: u64,
    interval_ns: u64,
    next_send_time: Instant,
}

impl RateLimiter {
    /// Create a new rate limiter with the specified target TPS.
    pub fn new(target_tps: u64) -> Self {
        let interval_ns = if target_tps > 0 {
            1_000_000_000 / target_tps
        } else {
            1_000_000_000 // Default to 1 TPS if 0 is specified
        };

        Self {
            target_tps,
            interval_ns,
            next_send_time: Instant::now(),
        }
    }

    /// Returns the target TPS for this rate limiter.
    pub fn target_tps(&self) -> u64 {
        self.target_tps
    }

    /// Wait until next scheduled send time, returning the scheduled time.
    ///
    /// This returns the *scheduled* time (not actual time) to support
    /// coordinated omission correction - if we fall behind, the scheduled
    /// time will be in the past but we still record it for accurate latency
    /// measurement.
    pub fn wait_for_next(&mut self) -> Instant {
        let scheduled_time = self.next_send_time;
        let now = Instant::now();

        if now < scheduled_time {
            std::thread::sleep(scheduled_time - now);
        }

        self.next_send_time += Duration::from_nanos(self.interval_ns);
        scheduled_time
    }

    /// Reset the rate limiter's timing to start fresh from now.
    pub fn reset(&mut self) {
        self.next_send_time = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_creation() {
        let limiter = RateLimiter::new(1000);
        assert_eq!(limiter.target_tps(), 1000);
    }

    #[test]
    fn test_rate_limiter_interval_calculation() {
        // 1000 TPS should give 1ms intervals
        let limiter = RateLimiter::new(1000);
        assert_eq!(limiter.interval_ns, 1_000_000);

        // 100 TPS should give 10ms intervals
        let limiter = RateLimiter::new(100);
        assert_eq!(limiter.interval_ns, 10_000_000);
    }

    #[test]
    fn test_rate_limiter_zero_tps_defaults_to_one() {
        let limiter = RateLimiter::new(0);
        // Should default to 1 second interval (1 TPS)
        assert_eq!(limiter.interval_ns, 1_000_000_000);
    }

    #[test]
    fn test_rate_limiter_wait_advances_time() {
        let mut limiter = RateLimiter::new(10000); // 10000 TPS, 0.1ms intervals
        let first_scheduled = limiter.wait_for_next();
        let second_scheduled = limiter.wait_for_next();

        // Second scheduled time should be after first
        assert!(second_scheduled > first_scheduled);

        // The difference should be approximately the interval
        let diff = second_scheduled.duration_since(first_scheduled);
        assert_eq!(diff.as_nanos(), 100_000); // 0.1ms = 100,000 ns
    }

    #[test]
    fn test_rate_limiter_reset() {
        let mut limiter = RateLimiter::new(1000);

        // Advance the limiter
        for _ in 0..10 {
            limiter.wait_for_next();
        }

        // Reset and check that next send time is close to now
        let before_reset = Instant::now();
        limiter.reset();
        let scheduled = limiter.wait_for_next();

        // Scheduled time should be very close to reset time
        let diff = scheduled.saturating_duration_since(before_reset);
        assert!(diff.as_millis() < 10);
    }
}
