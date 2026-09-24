#[derive(Debug, Clone)]
pub struct RollingAverage {
    values: Vec<f64>,
    next: usize,
    total: f64,
}

impl RollingAverage {
    pub fn new(window: usize) -> Self {
        assert!(window > 0, "rolling window must be positive");
        Self { values: vec![0.0; window], next: 0, total: 0.0 }
    }

    pub fn push(&mut self, value: f64) -> f64 {
        self.total -= self.values[self.next];
        self.values[self.next] = value;
        self.total += value;
        self.next = (self.next + 1) % self.values.len();
        self.average()
    }

    pub fn average(&self) -> f64 {
        self.total / self.values.len() as f64
    }

    pub fn window(&self) -> usize {
        self.values.len()
    }
}

#[cfg(test)]
mod tests {
    use super::RollingAverage;

    #[test]
    fn replaces_the_oldest_sample() {
        let mut average = RollingAverage::new(3);
        average.push(3.0);
        average.push(6.0);
        average.push(9.0);
        assert_eq!(average.push(12.0), 9.0);
    }
}
