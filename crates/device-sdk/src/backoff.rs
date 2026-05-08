#[derive(Debug, Clone)]
pub struct RetryBackoff {
    pub initial_seconds: u64,
    pub max_seconds: u64,
}

impl RetryBackoff {
    pub fn new(initial_seconds: u64, max_seconds: u64) -> Self {
        Self {
            initial_seconds: initial_seconds.max(1),
            max_seconds: max_seconds.max(initial_seconds),
        }
    }

    /// 指数退避 + 抖动（jitter），不依赖随机数库
    ///
    /// 行为：
    /// - 第一次返回 initial
    /// - 每次翻倍，最大不超过 max_seconds
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
