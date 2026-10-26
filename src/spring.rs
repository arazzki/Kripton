//! Damped harmonic oscillator used to drive the notch geometry.
//!
//! The PRD specifies a slightly underdamped spring (zeta ~= 0.72) instead of
//! CSS-style easing so the notch can reverse direction mid-flight when the
//! pointer enters or leaves the target area.

#[derive(Debug, Clone, Copy)]
pub struct SpringConfig {
    /// Natural frequency in radians per second.
    pub omega: f32,
    /// Damping ratio. 1.0 is critically damped; below 1.0 overshoots.
    pub zeta: f32,
}

impl Default for SpringConfig {
    fn default() -> Self {
        // 17 rad/s settles in roughly 350 ms. At zeta 0.72 the overshoot is
        // about 3.5%, which reads as the soft Apple-style bounce from the PRD.
        Self {
            omega: 17.0,
            zeta: 0.72,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
}

impl Spring {
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Advance one tick. Returns `true` while the spring is still moving.
    pub fn step(&mut self, dt: f32, config: SpringConfig) -> bool {
        // Clamp dt so a stalled event loop cannot inject energy and explode
        // the semi-implicit integrator.
        let dt = dt.clamp(0.001, 0.05);
        let displacement = self.value - self.target;
        let acceleration = -config.omega * config.omega * displacement
            - 2.0 * config.zeta * config.omega * self.velocity;
        self.velocity += acceleration * dt;
        self.value += self.velocity * dt;

        if (self.target - self.value).abs() < 0.05 && self.velocity.abs() < 0.5 {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(spring: &mut Spring, seconds: f32) -> f32 {
        let config = SpringConfig::default();
        let step = 1.0 / 60.0;
        let mut peak: f32 = spring.value();
        for _ in 0..(seconds / step) as usize {
            spring.step(step, config);
            peak = peak.max(spring.value());
        }
        peak
    }

    #[test]
    fn spring_settles_on_target() {
        let mut spring = Spring::new(190.0);
        spring.set_target(460.0);
        run(&mut spring, 1.5);
        assert!((spring.value() - 460.0).abs() < 0.01);
        assert_eq!(spring.target, 460.0);
    }

    #[test]
    fn spring_overshoots_softly() {
        let mut spring = Spring::new(190.0);
        spring.set_target(460.0);
        let peak = run(&mut spring, 1.5);
        let overshoot = (peak - 460.0) / 270.0;
        assert!(
            overshoot > 0.005,
            "expected a visible bounce, got {overshoot}"
        );
        assert!(overshoot < 0.09, "overshoot too large: {overshoot}");
    }

    #[test]
    fn spring_retargets_mid_flight() {
        let mut spring = Spring::new(190.0);
        spring.set_target(460.0);
        run(&mut spring, 0.12);
        spring.set_target(190.0);
        run(&mut spring, 1.5);
        assert!((spring.value() - 190.0).abs() < 0.01);
    }
}
