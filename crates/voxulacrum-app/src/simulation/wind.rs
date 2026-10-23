use glam::Vec2;

use crate::params::WindParams;

pub struct WindState {
    pub wind_vector: Vec2,
}

impl WindState {
    pub fn new() -> Self {
        Self {
            wind_vector: Vec2::new(1.0, 0.5),
        }
    }

    /// Set the wind for `seconds` of simulation time. A function of time
    /// alone, so nothing accumulates.
    pub fn update(&mut self, seconds: f32, params: &WindParams) {
        let t = seconds;

        let base_angle = t * params.direction_frequency;
        let base_magnitude = params.base_magnitude
            + params.magnitude_variation * (t * params.magnitude_frequency).sin();

        let gust = ((t * params.gust_frequency_a).sin()
            * (t * params.gust_frequency_b).sin())
            .max(0.0) * params.gust_strength;

        self.wind_vector = Vec2::new(base_angle.cos(), base_angle.sin())
            * (base_magnitude + gust);
    }
}