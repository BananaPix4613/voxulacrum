use glam::{Mat4, Vec3};
use winit::event::{ElementState, MouseScrollDelta};
use winit::keyboard::{KeyCode, PhysicalKey};

pub struct IsometricCamera {
    pub target: Vec3,
    pub zoom: f32,
    pub rotation: f32,
    pub aspect: f32,
    pan_speed: f32,
    forward_pressed: bool,
    backward_pressed: bool,
    left_pressed: bool,
    right_pressed: bool,
}

impl IsometricCamera {
    pub fn new() -> Self {
        Self {
            target: Vec3::new(64.0, 32.0, 64.0),
            zoom: 40.0,
            rotation: std::f32::consts::FRAC_PI_4,
            aspect: 1.0,
            pan_speed: 12.0,
            forward_pressed: false,
            backward_pressed: false,
            left_pressed: false,
            right_pressed: false,
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        let pitch = (1.0_f32 / 2.0_f32.sqrt()).atan();
        let direction = Vec3::new(
            self.rotation.cos() * pitch.cos(),
            pitch.sin(),
            self.rotation.sin() * pitch.cos(),
        );
        let eye = self.target + direction * 150.0;
        Mat4::look_at_rh(eye, self.target, Vec3::Y)
    }

    pub fn projection_matrix(&self) -> Mat4 {
        let half_height = self.zoom;
        let half_width = half_height * self.aspect;
        Mat4::orthographic_rh(
            -half_width,
            half_width,
            -half_height,
            half_height,
            0.1,
            300.0,
        )
    }

    pub fn view_projection(&self) -> [[f32; 4]; 4] {
        (self.projection_matrix() * self.view_matrix()).to_cols_array_2d()
    }

    pub fn process_keyboard(&mut self, key: PhysicalKey, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if let PhysicalKey::Code(code) = key {
            match code {
                KeyCode::KeyW | KeyCode::ArrowUp => self.forward_pressed = pressed,
                KeyCode::KeyS | KeyCode::ArrowDown => self.backward_pressed = pressed,
                KeyCode::KeyA | KeyCode::ArrowLeft => self.left_pressed = pressed,
                KeyCode::KeyD | KeyCode::ArrowRight => self.right_pressed = pressed,
                _ => {}
            }
        }
    }

    pub fn process_scroll(&mut self, delta: &MouseScrollDelta) {
        let scroll = match delta {
            MouseScrollDelta::LineDelta(_, y) => *y,
            MouseScrollDelta::PixelDelta(pos) => pos.y as f32 * 0.1,
        };
        self.zoom = (self.zoom - scroll * 2.0).clamp(5.0, 60.0);
    }

    pub fn update(&mut self, dt: f32) {
        let forward_dir = Vec3::new(-self.rotation.cos(), 0.0, -self.rotation.sin());
        let right_dir = Vec3::new(self.rotation.sin(), 0.0, -self.rotation.cos());

        let mut move_dir = Vec3::ZERO;
        if self.forward_pressed {
            move_dir += forward_dir;
        }
        if self.backward_pressed {
            move_dir -= forward_dir;
        }
        if self.left_pressed {
            move_dir -= right_dir;
        }
        if self.right_pressed {
            move_dir += right_dir;
        }

        if move_dir.length_squared() > 0.0 {
            move_dir = move_dir.normalize();
        }

        self.target += move_dir * self.pan_speed * dt;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if height > 0 {
            self.aspect = width as f32 / height as f32;
        }
    }
}