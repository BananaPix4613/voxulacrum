use glam::Vec2;

pub struct WindState {
    pub wind_vector: Vec2,
    elapsed: f32,
}

impl WindState {
    pub fn new() -> Self {
        Self {
            wind_vector: Vec2::new(1.0, 0.5),
            elapsed: 0.0,
        }
    }
    
    pub fn update(&mut self, dt: f32) {
        self.elapsed += dt;
        let t = self.elapsed;
        
        let base_angle = t * 0.02;
        let base_magnitude = 2.0 + 1.0 * (t * 0.03).sin();
        
        let gust = ((t * 0.7).sin() * (t * 1.3).sin()).max(0.0) * 3.0;
        
        self.wind_vector = Vec2::new(base_angle.cos(), base_angle.sin())
            * (base_magnitude + gust);
    }
}