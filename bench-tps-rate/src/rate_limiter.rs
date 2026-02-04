use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::time::{Duration, Instant};

/// Rate limiter for controlling transaction send rate.
///
/// Uses an exponential distribution (Poisson arrival process) with
/// mean interval = 1,000,000,000 ns / target_tps for realistic traffic patterns.
pub struct RateLimiter {
    target_tps: u64,
    mean_interval_ns: f64,
    next_send_time: Instant,
    rng: SmallRng,
}

impl RateLimiter {
    /// Create a new rate limiter with the specified target TPS.
    pub fn new(target_tps: u64) -> Self {
        let mean_interval_ns = if target_tps > 0 {
            1_000_000_000.0 / target_tps as f64
        } else {
            1_000_000_000.0 // Default to 1 TPS if 0 is specified
        };

        Self {
            target_tps,
            mean_interval_ns,
            next_send_time: Instant::now(),
            rng: SmallRng::from_entropy(),
        }
    }

    /// Returns the target TPS for this rate limiter.
    pub fn target_tps(&self) -> u64 {
        self.target_tps
    }

    /// Sample from exponential distribution using inverse transform sampling.
    /// Returns interval in nanoseconds.
    fn sample_exponential_ns(&mut self) -> u64 {
        // U ~ Uniform(0,1), excluding 0 to avoid -ln(0) = infinity
        let u: f64 = self.rng.gen_range(f64::MIN_POSITIVE..1.0);

        // T = -ln(U) * mean_interval (inverse transform sampling)
        let interval_ns = -u.ln() * self.mean_interval_ns;

        // Clamp: min 1ns, max 10x mean (covers 99.995% of distribution)
        interval_ns.clamp(1_000.0, self.mean_interval_ns * 20.0) as u64
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

        // Use exponential instead of fixed interval
        let next_interval_ns = self.sample_exponential_ns();
        self.next_send_time += Duration::from_nanos(next_interval_ns);

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
    fn test_rate_limiter_zero_tps_defaults_to_one() {
        let limiter = RateLimiter::new(0);
        // Should default to 1 second mean interval (1 TPS)
        assert!((limiter.mean_interval_ns - 1_000_000_000.0).abs() < 1.0);
    }

    #[test]
    fn test_exponential_mean_approximates_target() {
        let mut limiter = RateLimiter::new(1000); // 1000 TPS, 1ms mean interval
        let expected_mean = 1_000_000.0; // 1ms in ns

        // Sample many intervals and check the mean
        let num_samples = 10_000;
        let sum: u64 = (0..num_samples)
            .map(|_| limiter.sample_exponential_ns())
            .sum();
        let actual_mean = sum as f64 / num_samples as f64;

        // Allow 10% tolerance for statistical variance
        let tolerance = expected_mean * 0.10;
        assert!(
            (actual_mean - expected_mean).abs() < tolerance,
            "Expected mean ~{}, got {} (tolerance {})",
            expected_mean,
            actual_mean,
            tolerance
        );
    }

    #[test]
    fn test_exponential_all_positive() {
        let mut limiter = RateLimiter::new(10000);

        // All samples should be at least 1ns
        for _ in 0..1000 {
            let interval = limiter.sample_exponential_ns();
            assert!(interval >= 1, "Interval {} should be >= 1", interval);
        }
    }

    #[test]
    fn test_exponential_bounded() {
        let mut limiter = RateLimiter::new(1000);
        let max_expected = (limiter.mean_interval_ns * 10.0) as u64;

        // All samples should be at most 10x mean
        for _ in 0..1000 {
            let interval = limiter.sample_exponential_ns();
            assert!(
                interval <= max_expected,
                "Interval {} should be <= {}",
                interval,
                max_expected
            );
        }
    }

    #[test]
    fn test_wait_for_next_advances_time() {
        let mut limiter = RateLimiter::new(10000); // 10000 TPS
        let first_scheduled = limiter.wait_for_next();
        let second_scheduled = limiter.wait_for_next();

        // Second scheduled time should be after first (time always advances)
        assert!(
            second_scheduled > first_scheduled,
            "Time should always advance"
        );
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
