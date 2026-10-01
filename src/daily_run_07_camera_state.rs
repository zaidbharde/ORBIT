#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraState {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl CameraState {
    pub fn new(distance: f32) -> Self {
        Self { yaw: 0.0, pitch: 0.0, distance: distance.max(0.1) }
    }

    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw = (self.yaw + delta_yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + delta_pitch).clamp(-1.5, 1.5);
    }

    pub fn zoom(&mut self, wheel_delta: f32) {
        let scale = (-wheel_delta * 0.1).exp();
        self.distance = (self.distance * scale).clamp(0.1, 10_000.0);
    }

    pub fn forward_vector(&self) -> [f32; 3] {
        let horizontal = self.pitch.cos();
        [horizontal * self.yaw.cos(), self.pitch.sin(), horizontal * self.yaw.sin()]
    }
}

#[cfg(test)]
mod tests {
    use super::CameraState;
    #[test]
    fn pitch_stays_inside_vertical_limits() {
        let mut camera = CameraState::new(5.0);
        camera.orbit(0.0, 4.0);
        assert_eq!(camera.pitch, 1.5);
    }
}
