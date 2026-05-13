#[derive(Debug, Clone)]
pub struct RetryBackoff {
    pub initial_seconds: u64,
    pub max_seconds: u64,
}

impl RetryBackoff {
    pub fn new(initial_seconds: u64, max_seconds: u64) -> Self {
        let initial_seconds = initial_seconds.max(1);
        let max_seconds = max_seconds.max(initial_seconds);

        Self {
            initial_seconds,
            max_seconds,
        }
    }

    /// 指数退避 + 抖动（jitter），不依赖随机数库
    ///
    /// 行为：
    /// - 基础退避值从 initial 开始，之后每次翻倍，最大不超过 max_seconds
    /// - 返回值（包括第一次）都会应用 jitter
    /// - jitter：在 [0.5x, 1.5x] 范围内扰动（通过 attempt 的确定性 hash 产生扰动）
    pub fn next_sleep_seconds(&self, attempt: u32) -> u64 {
        let base = self
            .initial_seconds
            .saturating_mul(2u64.saturating_pow(attempt.min(30)));
        let capped = base.min(self.max_seconds);

        // Deterministic jitter in [0.5, 1.5).
        // Using a simple integer hash to avoid pulling rand as dependency.
        let x = attempt as u64;
        let hashed = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let frac = (hashed % 1000) as f64 / 1000.0; // [0, 1)
        let jitter = 0.5 + frac; // [0.5, 1.5)

        let with_jitter = (capped as f64 * jitter).round();
        with_jitter.clamp(1.0, self.max_seconds as f64) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_initial_is_one() {
        let b = RetryBackoff::new(1, 60);
        let sleep = b.next_sleep_seconds(0);
        assert!(sleep >= 1, "first attempt should be >= 1s, got {sleep}");
    }

    #[test]
    fn backoff_increases_with_attempts() {
        let b = RetryBackoff::new(1, 60);
        let s1 = b.next_sleep_seconds(0);
        let s3 = b.next_sleep_seconds(3);
        assert!(
            s3 >= s1,
            "later attempts should have longer wait, got {s1} -> {s3}"
        );
    }

    #[test]
    fn backoff_max_delay_capped() {
        let b = RetryBackoff::new(1, 10);
        let s30 = b.next_sleep_seconds(30);
        let s50 = b.next_sleep_seconds(50);
        assert!(
            s30 <= 15, // max 10 * jitter (~1.5)
            "s30 should be capped near max=10, got {s30}"
        );
        assert!(s50 <= 15, "s50 should also be capped, got {s50}");
    }

    #[test]
    fn backoff_jitter_produces_different_values() {
        let b = RetryBackoff::new(10, 60);
        // Same attempt should give same deterministic jitter
        let v1 = b.next_sleep_seconds(5);
        let v2 = b.next_sleep_seconds(5);
        assert_eq!(v1, v2, "same attempt should produce same jitter");

        // Different attempts should give different values
        let v3 = b.next_sleep_seconds(6);
        assert_ne!(v1, v3, "different attempts should give different jitter");
    }

    #[test]
    fn backoff_constructor_clamps_values() {
        let b = RetryBackoff::new(0, 0);
        assert_eq!(b.initial_seconds, 1);
        assert_eq!(b.max_seconds, 1);

        let b = RetryBackoff::new(5, 3);
        assert_eq!(b.initial_seconds, 5);
        assert_eq!(b.max_seconds, 5);
    }
}
