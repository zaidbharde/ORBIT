#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdState {
    Normal,
    Warning,
    Breached,
}

#[derive(Debug, Clone, Copy)]
pub struct ThresholdTracker {
    warning: f64,
    breach: f64,
    state: ThresholdState,
}

impl ThresholdTracker {
    pub fn new(warning: f64, breach: f64) -> Self {
        assert!(warning <= breach, "warning threshold must not exceed breach threshold");
        Self { warning, breach, state: ThresholdState::Normal }
    }

    pub fn observe(&mut self, value: f64) -> ThresholdState {
        self.state = if value >= self.breach {
            ThresholdState::Breached
        } else if value >= self.warning {
            ThresholdState::Warning
        } else {
            ThresholdState::Normal
        };
        self.state
    }

    pub fn state(&self) -> ThresholdState {
        self.state
    }
}
