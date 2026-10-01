pub struct FrameSmoother {
    average_ms: f32,
    alpha: f32,
    initialized: bool,
}

impl FrameSmoother {
    pub fn new(window_samples: usize) -> Self {
        let samples = window_samples.max(1) as f32;
        Self { average_ms: 0.0, alpha: 2.0 / (samples + 1.0), initialized: false }
    }

    pub fn record(&mut self, frame_seconds: f32) -> f32 {
        let milliseconds = (frame_seconds.max(0.0) * 1_000.0).min(10_000.0);
        if !self.initialized {
            self.average_ms = milliseconds;
            self.initialized = true;
        } else {
            self.average_ms += self.alpha * (milliseconds - self.average_ms);
        }
        self.average_ms
    }

    pub fn fps(&self) -> Option<f32> {
        if self.average_ms > 0.0 { Some(1_000.0 / self.average_ms) } else { None }
    }

    pub fn reset(&mut self) { self.average_ms = 0.0; self.initialized = false; }
}

#[cfg(test)]
mod tests {
    use super::FrameSmoother;
    #[test]
    fn first_sample_initializes_the_average() {
        let mut smoother = FrameSmoother::new(8);
        assert_eq!(smoother.record(0.016), 16.0);
        assert!((smoother.fps().unwrap() - 62.5).abs() < 0.01);
    }
}
