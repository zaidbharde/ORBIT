//! Exponential backoff calculator with a reproducible pseudo-random jitter factor.

#[derive(Debug, Clone, Copy)]
pub struct Backoff { base_ms: u64, cap_ms: u64, attempt: u32, state: u64 }

impl Backoff {
    pub fn new(base_ms: u64, cap_ms: u64, seed: u64) -> Result<Self, &'static str> {
        if base_ms == 0 || cap_ms < base_ms { return Err("backoff bounds are invalid"); }
        Ok(Self { base_ms, cap_ms, attempt: 0, state: seed.max(1) })
    }

    pub fn next_delay_ms(&mut self) -> u64 {
        let shift = self.attempt.min(31);
        let exponential = self.base_ms.saturating_mul(1u64 << shift).min(self.cap_ms);
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let jitter = exponential / 5;
        let offset = if jitter == 0 { 0 } else { self.state % (jitter + 1) };
        self.attempt = self.attempt.saturating_add(1);
        exponential.saturating_sub(jitter / 2).saturating_add(offset).min(self.cap_ms)
    }

    pub fn reset(&mut self) { self.attempt = 0; }
    pub fn attempts(&self) -> u32 { self.attempt }
}
