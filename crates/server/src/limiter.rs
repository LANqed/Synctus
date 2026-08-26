//! Token-bucket rate limiting for relayed frames.
//!
//! Per connection, not per room: one misbehaving device must not slow down its
//! peer. Uses `Instant` deltas rather than a background timer, so an idle
//! connection costs nothing.

use std::time::Instant;

pub struct RateLimiter {
    /// Tokens added per second.
    rate: f64,
    /// Maximum tokens held.
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(per_sec: u32, burst: u32) -> Self {
        let rate = per_sec.max(1) as f64;
        let burst = burst.max(per_sec).max(1) as f64;
        Self {
            rate,
            burst,
            // Start full so a client can publish its initial state immediately.
            tokens: burst,
            last: Instant::now(),
        }
    }

    /// Take one token. `false` means the caller should drop the frame.
    pub fn allow(&mut self) -> bool {
        self.refill(Instant::now());
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
            self.last = now;
        }
    }

    /// Test seam: advance the clock without sleeping.
    #[cfg(test)]
    fn advance(&mut self, by: std::time::Duration) {
        self.last -= by;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn burst_is_available_immediately() {
        let mut rl = RateLimiter::new(10, 30);
        for i in 0..30 {
            assert!(rl.allow(), "token {i} should be available");
        }
        assert!(!rl.allow(), "burst must be exhausted");
    }

    #[test]
    fn tokens_refill_over_time() {
        let mut rl = RateLimiter::new(10, 10);
        while rl.allow() {}
        rl.advance(Duration::from_millis(500));
        // 10/s for 0.5 s = 5 tokens.
        for _ in 0..5 {
            assert!(rl.allow());
        }
        assert!(!rl.allow());
    }

    #[test]
    fn refill_is_capped_at_burst() {
        let mut rl = RateLimiter::new(10, 20);
        while rl.allow() {}
        rl.advance(Duration::from_secs(3600));
        let mut granted = 0;
        while rl.allow() {
            granted += 1;
        }
        assert_eq!(granted, 20, "must not accumulate beyond the burst");
    }

    #[test]
    fn burst_is_never_below_the_rate() {
        // A misconfigured burst smaller than the rate would throttle below the
        // configured steady state.
        let mut rl = RateLimiter::new(10, 1);
        let mut granted = 0;
        while rl.allow() {
            granted += 1;
        }
        assert_eq!(granted, 10);
    }
}
