use pingora_limits::rate::Rate;
use std::sync::Arc;
use std::time::Duration;

use crate::config::RateLimitConfig;

/// Rate limiter using pingora's sliding-window Rate estimator.
///
/// Uses `observe()` which returns the count of events in the current window.
/// We compare against the max allowed events per window to decide if limited.
pub struct RateLimiter {
    ip_limiter: Arc<Rate>,
    user_limiter: Arc<Rate>,
    ip_max_per_window: isize,
    user_max_per_window: isize,
}

impl RateLimiter {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            ip_limiter: Arc::new(Rate::new(Duration::from_secs(1))),
            user_limiter: Arc::new(Rate::new(Duration::from_secs(1))),
            ip_max_per_window: config.ip_requests_per_second,
            user_max_per_window: config.user_requests_per_second,
        }
    }

    pub fn check_ip(&self, ip: &str) -> RateLimitResult {
        let key = format!("ip:{}", ip);
        let current = self.ip_limiter.observe(&key, 1);
        if current > self.ip_max_per_window {
            RateLimitResult::Limited { current, limit: self.ip_max_per_window }
        } else {
            RateLimitResult::Allowed { current }
        }
    }

    pub fn check_user(&self, user_id: &str) -> RateLimitResult {
        let key = format!("user:{}", user_id);
        let current = self.user_limiter.observe(&key, 1);
        if current > self.user_max_per_window {
            RateLimitResult::Limited { current, limit: self.user_max_per_window }
        } else {
            RateLimitResult::Allowed { current }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RateLimitResult {
    Allowed { current: isize },
    Limited { current: isize, limit: isize },
}

impl RateLimitResult {
    pub fn is_limited(&self) -> bool {
        matches!(self, Self::Limited { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(ip_rps: isize, user_rps: isize) -> RateLimitConfig {
        RateLimitConfig {
            ip_requests_per_second: ip_rps,
            user_requests_per_second: user_rps,
            burst_multiplier: 1,
        }
    }

    #[test]
    fn allows_under_limit() {
        let limiter = RateLimiter::new(&test_config(10, 5));

        let result = limiter.check_ip("192.168.1.1");
        assert!(!result.is_limited());
    }

    #[test]
    fn limits_ip_over_threshold() {
        let limiter = RateLimiter::new(&test_config(3, 10));

        // First 3 should be allowed
        assert!(!limiter.check_ip("10.0.0.1").is_limited());
        assert!(!limiter.check_ip("10.0.0.1").is_limited());
        assert!(!limiter.check_ip("10.0.0.1").is_limited());

        // 4th should be limited
        assert!(limiter.check_ip("10.0.0.1").is_limited());
    }

    #[test]
    fn limits_user_over_threshold() {
        let limiter = RateLimiter::new(&test_config(100, 2));

        assert!(!limiter.check_user("user-abc").is_limited());
        assert!(!limiter.check_user("user-abc").is_limited());

        // 3rd request exceeds limit of 2
        assert!(limiter.check_user("user-abc").is_limited());
    }

    #[test]
    fn different_keys_independent() {
        let limiter = RateLimiter::new(&test_config(2, 2));

        // Exhaust limit for IP A
        limiter.check_ip("10.0.0.1");
        limiter.check_ip("10.0.0.1");
        assert!(limiter.check_ip("10.0.0.1").is_limited());

        // IP B should still be allowed
        assert!(!limiter.check_ip("10.0.0.2").is_limited());
    }
}
