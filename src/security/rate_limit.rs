//! Rate limiting for Git operations.
//!
//! This module prevents runaway AI operations by limiting the rate
//! of Git commands that can be executed.
//!
//! # Algorithm
//!
//! Uses a token bucket algorithm:
//! - Bucket starts with `max_burst` tokens
//! - Each operation consumes one token
//! - Tokens are replenished at `refill_rate` per second
//! - If no tokens available, operation is blocked

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Rate limiter using token bucket algorithm.
#[derive(Debug)]
pub struct RateLimiter {
    /// Maximum tokens in the bucket (burst capacity).
    max_burst: u64,

    /// Current tokens available.
    tokens: Mutex<f64>,

    /// Tokens added per second.
    refill_rate: f64,

    /// Last time tokens were refilled.
    last_refill: Mutex<Instant>,

    /// Total operations allowed (lifetime).
    total_allowed: AtomicU64,

    /// Total operations blocked (lifetime).
    total_blocked: AtomicU64,
}

impl RateLimiter {
    /// Creates a new rate limiter.
    ///
    /// # Arguments
    ///
    /// * `max_burst` — Maximum operations allowed in a burst
    /// * `refill_rate` — Operations allowed per second (sustained rate)
    ///
    /// # Example
    ///
    /// ```
    /// use git_proxy_mcp::security::RateLimiter;
    ///
    /// // Allow burst of 10 operations, sustained rate of 2/second
    /// let limiter = RateLimiter::new(10, 2.0);
    /// ```
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // max_burst as f64 is acceptable
    pub fn new(max_burst: u64, refill_rate: f64) -> Self {
        Self {
            max_burst,
            tokens: Mutex::new(max_burst as f64),
            refill_rate,
            last_refill: Mutex::new(Instant::now()),
            total_allowed: AtomicU64::new(0),
            total_blocked: AtomicU64::new(0),
        }
    }

    /// Creates a rate limiter with sensible defaults for AI operations.
    ///
    /// Default: 20 burst, 5 operations/second
    #[must_use]
    pub fn default_for_ai() -> Self {
        Self::new(20, 5.0)
    }

    /// Creates a rate limiter that allows unlimited operations.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::new(u64::MAX, f64::MAX)
    }

    /// Attempts to acquire a token for an operation.
    ///
    /// Returns `true` if the operation is allowed, `false` if rate limited.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[allow(clippy::significant_drop_tightening)] // Lock must be held during update
    pub fn try_acquire(&self) -> bool {
        self.refill();

        let mut tokens = self.tokens.lock().unwrap();

        if *tokens >= 1.0 {
            *tokens -= 1.0;
            drop(tokens);
            self.total_allowed.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            drop(tokens);
            self.total_blocked.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Checks if an operation would be allowed without consuming a token.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn would_allow(&self) -> bool {
        self.refill();
        let tokens = self.tokens.lock().unwrap();
        *tokens >= 1.0
    }

    /// Returns the current number of available tokens.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn available_tokens(&self) -> f64 {
        self.refill();
        let tokens = self.tokens.lock().unwrap();
        *tokens
    }

    /// Returns time until the next token is available.
    ///
    /// Returns `Duration::ZERO` if tokens are currently available.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn time_until_available(&self) -> Duration {
        self.refill();

        let tokens = self.tokens.lock().unwrap();
        let current_tokens = *tokens;
        drop(tokens);

        if current_tokens >= 1.0 {
            Duration::ZERO
        } else {
            let tokens_needed = 1.0 - current_tokens;
            let seconds = tokens_needed / self.refill_rate;
            Duration::from_secs_f64(seconds)
        }
    }

    /// Refills tokens based on elapsed time.
    #[allow(clippy::significant_drop_tightening)] // Lock ordering is intentional
    #[allow(clippy::cast_precision_loss)] // max_burst as f64 is acceptable
    fn refill(&self) {
        let now = Instant::now();

        let mut last_refill = self.last_refill.lock().unwrap();
        let elapsed = now.duration_since(*last_refill);

        if elapsed.as_secs_f64() > 0.0 {
            let mut tokens = self.tokens.lock().unwrap();

            let new_tokens = elapsed.as_secs_f64() * self.refill_rate;
            *tokens = (*tokens + new_tokens).min(self.max_burst as f64);

            *last_refill = now;
        }
    }

    /// Returns statistics about rate limiting.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn stats(&self) -> RateLimiterStats {
        RateLimiterStats {
            total_allowed: self.total_allowed.load(Ordering::Relaxed),
            total_blocked: self.total_blocked.load(Ordering::Relaxed),
            available_tokens: self.available_tokens(),
            max_burst: self.max_burst,
            refill_rate: self.refill_rate,
        }
    }

    /// Resets the rate limiter to its initial state.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[allow(clippy::significant_drop_tightening)] // Locks are independent
    #[allow(clippy::cast_precision_loss)] // max_burst as f64 is acceptable
    pub fn reset(&self) {
        let mut tokens = self.tokens.lock().unwrap();
        *tokens = self.max_burst as f64;
        drop(tokens);

        let mut last_refill = self.last_refill.lock().unwrap();
        *last_refill = Instant::now();
        drop(last_refill);

        self.total_allowed.store(0, Ordering::Relaxed);
        self.total_blocked.store(0, Ordering::Relaxed);
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::default_for_ai()
    }
}

/// Statistics about rate limiter usage.
#[derive(Debug, Clone, Copy)]
pub struct RateLimiterStats {
    /// Total operations that were allowed.
    pub total_allowed: u64,

    /// Total operations that were blocked.
    pub total_blocked: u64,

    /// Currently available tokens.
    pub available_tokens: f64,

    /// Maximum burst capacity.
    pub max_burst: u64,

    /// Tokens per second (sustained rate).
    pub refill_rate: f64,
}

impl RateLimiterStats {
    /// Returns the percentage of operations that were blocked.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Percentage calculation is acceptable
    pub fn block_rate(&self) -> f64 {
        let total = self.total_allowed + self.total_blocked;
        if total == 0 {
            0.0
        } else {
            (self.total_blocked as f64 / total as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn rate_limiter_allows_within_burst() {
        let limiter = RateLimiter::new(5, 1.0);

        // Should allow 5 operations
        for _ in 0..5 {
            assert!(limiter.try_acquire());
        }

        // 6th should be blocked
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn rate_limiter_refills_over_time() {
        let limiter = RateLimiter::new(2, 10.0); // 10 tokens/second

        // Exhaust tokens
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());

        // Wait for refill (100ms = 1 token at 10/sec)
        thread::sleep(Duration::from_millis(150));

        // Should have refilled
        assert!(limiter.try_acquire());
    }

    #[test]
    fn rate_limiter_caps_at_max_burst() {
        let limiter = RateLimiter::new(3, 100.0);

        // Wait for tokens to accumulate
        thread::sleep(Duration::from_millis(100));

        // Should still only have max_burst tokens
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn rate_limiter_would_allow_doesnt_consume() {
        let limiter = RateLimiter::new(1, 0.0);

        assert!(limiter.would_allow());
        assert!(limiter.would_allow()); // Still true

        assert!(limiter.try_acquire()); // Now consume
        assert!(!limiter.would_allow()); // Now false
    }

    #[test]
    fn rate_limiter_time_until_available() {
        let limiter = RateLimiter::new(1, 10.0);

        // With tokens available
        assert_eq!(limiter.time_until_available(), Duration::ZERO);

        // Exhaust tokens
        limiter.try_acquire();

        // Should need ~100ms for next token
        let wait_time = limiter.time_until_available();
        assert!(wait_time.as_millis() > 0);
        assert!(wait_time.as_millis() <= 150); // Some tolerance
    }

    #[test]
    fn rate_limiter_stats() {
        let limiter = RateLimiter::new(3, 1.0);

        limiter.try_acquire(); // allowed
        limiter.try_acquire(); // allowed
        limiter.try_acquire(); // allowed
        limiter.try_acquire(); // blocked
        limiter.try_acquire(); // blocked

        let stats = limiter.stats();
        assert_eq!(stats.total_allowed, 3);
        assert_eq!(stats.total_blocked, 2);
        assert_eq!(stats.max_burst, 3);
        assert!((stats.block_rate() - 40.0).abs() < 0.01);
    }

    #[test]
    fn rate_limiter_reset() {
        let limiter = RateLimiter::new(2, 0.0);

        // Exhaust and block some
        limiter.try_acquire();
        limiter.try_acquire();
        limiter.try_acquire();

        // Reset
        limiter.reset();

        let stats = limiter.stats();
        assert_eq!(stats.total_allowed, 0);
        assert_eq!(stats.total_blocked, 0);
        assert!(limiter.try_acquire());
    }

    #[test]
    fn rate_limiter_unlimited() {
        let limiter = RateLimiter::unlimited();

        // Should allow many operations
        for _ in 0..1000 {
            assert!(limiter.try_acquire());
        }
    }

    #[test]
    fn rate_limiter_default_for_ai() {
        let limiter = RateLimiter::default_for_ai();

        // Should have reasonable defaults
        assert_eq!(limiter.max_burst, 20);
        assert!((limiter.refill_rate - 5.0).abs() < 0.01);
    }

    #[test]
    fn block_rate_with_no_operations() {
        let limiter = RateLimiter::new(10, 1.0);
        let stats = limiter.stats();

        assert!((stats.block_rate() - 0.0).abs() < f64::EPSILON);
    }
}
