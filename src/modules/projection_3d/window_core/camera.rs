use cgmath::*;
use winit::event::MouseScrollDelta;
use winit::keyboard::*;
use winit::dpi::PhysicalPosition;
use std::time::Duration;
use std::f32::consts::FRAC_PI_2;

#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::from_cols(
    cgmath::Vector4::new(1.0, 0.0, 0.0, 0.0),
    cgmath::Vector4::new(0.0, 1.0, 0.0, 0.0),
    cgmath::Vector4::new(0.0, 0.0, 0.5, 0.0),
    cgmath::Vector4::new(0.0, 0.0, 0.5, 1.0),
);

const ASTRO_TO_GRAPHICS_MATRIX: Matrix3<f32> = Matrix3::from_cols(
    Vector3::new(1.0, 0.0, 0.0),
    Vector3::new(0.0, 0.0, -1.0),
    Vector3::new(0.0, 1.0, 0.0),
);

const SAFE_FRAC_PI_2: f32 = FRAC_PI_2 - 0.0001;

fn infinite_perspective_reverse_z(fovy: Rad<f32>, aspect: f32, znear: f32) -> Matrix4<f32> {
    let f = 1.0 / (fovy.0 / 2.0).tan();
    
    Matrix4::new(
        f / aspect, 0.0, 0.0, 0.0,
        0.0, f, 0.0, 0.0,
        0.0, 0.0, 0.0, -1.0,  
        0.0, 0.0, znear, 0.0,
    )
}

#[derive(Debug)]
pub struct Camera {
    pub position: Point3<f32>,
    orientation: Quaternion<f32>,
    pub distance: f32,
    pub target: Option<Point3<f32>>, // When set, camera looks at target
}

impl Camera {
    pub fn new<
        V: Into<Point3<f32>>,
        Y: Into<Rad<f32>>,
        P: Into<Rad<f32>>,
    >(
        position: V,
        yaw: Y,
        pitch: P,
    ) -> Self {
        let pos: Point3<f32> = position.into();
        let yaw: Rad<f32> = yaw.into();
        let pitch: Rad<f32> = pitch.into();
        let q_yaw = Quaternion::from_axis_angle(Vector3::unit_y(), yaw);
        let q_pitch = Quaternion::from_axis_angle(Vector3::unit_x(), pitch);
        let orientation = q_yaw * q_pitch;
        let distance = pos.to_vec().magnitude();
        Self { position: pos, orientation, distance, target: None }
    }

    pub fn calc_matrix(&self) -> Matrix4<f32> {
        if let Some(target) = self.target {
            Matrix4::look_at_rh(self.position, target, Vector3::unit_y())
        } else {
            let forward = self.orientation.rotate_vector(-Vector3::unit_z());
            Matrix4::look_to_rh(self.position, forward.normalize(), Vector3::unit_y())
        }
    }
}

pub struct Projection {
    aspect: f32,
    fovy: Rad<f32>,
    znear: f32,
   // zfar: f32,
}

impl Projection {
    pub fn new<F: Into<Rad<f32>>>(
        width: u32,
        height: u32,
        fovy: F,
        znear: f32,
       // zfar: f32,
    ) -> Self {
        Self {
            aspect: width as f32 / height as f32,
            fovy: fovy.into(),
            znear,
            //zfar,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.aspect = width as f32 / height as f32;
    }

    pub fn calc_matrix(&self) -> Matrix4<f32> {
        //OPENGL_TO_WGPU_MATRIX * perspective(self.fovy, self.aspect, self.znear, self.zfar)
        OPENGL_TO_WGPU_MATRIX * infinite_perspective_reverse_z(self.fovy, self.aspect, self.znear)
    }

    pub fn fov_deg(&self) -> f32 {
        self.fovy.0.to_degrees()
    }

    pub fn set_fov_deg(&mut self, deg: f32) {
        let clamped = deg.clamp(15.0, 100.0);
        self.fovy = Deg(clamped).into();
    }
}

#[derive(Debug)]
pub struct CameraController {
    amount_left: f32,
    amount_right: f32,
    amount_forward: f32,
    amount_backward: f32,
    amount_up: f32,
    amount_down: f32,
    rotate_horizontal: f32,
    rotate_vertical: f32,
    scroll: f32,
    speed: f32,
    sensitivity: f32,
}

impl CameraController {
    pub fn new(speed: f32, sensitivity: f32) -> Self {
        Self {
            amount_left: 0.0,
            amount_right: 0.0,
            amount_forward: 0.0,
            amount_backward: 0.0,
            amount_up: 0.0,
            amount_down: 0.0,
            rotate_horizontal: 0.0,
            rotate_vertical: 0.0,
            scroll: 0.0,
            speed,
            sensitivity,
        }
    }

   pub fn handle_key(&mut self, key: KeyCode, pressed: bool) -> bool {
        let amount = if pressed {
            1.0
        } else {
            0.0
        };
        match key {
            KeyCode::KeyW | KeyCode::ArrowUp => {
                self.amount_forward = amount;
                true
            }
            KeyCode::KeyS | KeyCode::ArrowDown => {
                self.amount_backward = amount;
                true
            }
            KeyCode::KeyA | KeyCode::ArrowLeft => {
                self.amount_left = amount;
                true
            }
            KeyCode::KeyD | KeyCode::ArrowRight => {
                self.amount_right = amount;
                true
            }
            KeyCode::Space => {
                self.amount_up = amount;
                true
            }
            KeyCode::ShiftLeft => {
                self.amount_down = amount;
                true
            }
            _ => false,
        }
    } 

    pub fn handle_mouse(&mut self, mouse_dx: f64, mouse_dy: f64) {
        self.rotate_horizontal = -mouse_dx as f32;
        self.rotate_vertical = -mouse_dy as f32;
    }

    pub fn handle_scroll(&mut self, delta: &MouseScrollDelta) {
        self.scroll = match delta {

            MouseScrollDelta::LineDelta(_, scroll) => scroll * 300.0,
            MouseScrollDelta::PixelDelta(PhysicalPosition {
                y: scroll,
                ..
            }) => *scroll as f32,
        };
    }

    pub fn update_camera(&mut self, camera: &mut Camera, dt: Duration) {
        let dt = dt.as_secs_f32();

        let forward = camera.orientation.rotate_vector(-Vector3::unit_z()).normalize();
        let right = camera.orientation.rotate_vector(Vector3::unit_x()).normalize();
        camera.position += forward * (self.amount_forward - self.amount_backward) * self.speed * dt;
        camera.position += right * (self.amount_right - self.amount_left) * self.speed * dt;

        camera.position += forward * self.scroll * self.speed * self.sensitivity * dt;
        self.scroll = 0.0;

        camera.position.y += (self.amount_up - self.amount_down) * self.speed * dt;

        let dyaw = Rad(self.rotate_horizontal) * self.sensitivity * dt;
        let dpitch = Rad(-self.rotate_vertical) * self.sensitivity * dt;
        if dyaw.0 != 0.0 || dpitch.0 != 0.0 {
            let q_yaw = Quaternion::from_axis_angle(Vector3::unit_y(), dyaw);
            let right = camera.orientation.rotate_vector(Vector3::unit_x()).normalize();
            let q_pitch = Quaternion::from_axis_angle(right, dpitch);
            camera.orientation = (q_yaw * q_pitch) * camera.orientation;
        }
        self.rotate_horizontal = 0.0;
        self.rotate_vertical = 0.0;
    }

    pub fn update_camera_orbital(
        &mut self,
        camera: &mut Camera,
        dt: Duration,
        target_pos: Point3<f32>,
        min_distance: f32,
    ) {
        let dt = dt.as_secs_f32();

        let target_graphics = Point3::from_vec(ASTRO_TO_GRAPHICS_MATRIX * target_pos.to_vec());
        camera.target = Some(target_graphics);

        let mut distance = (camera.position - target_graphics).magnitude();

        let dyaw = Rad(self.rotate_horizontal) * self.sensitivity * dt;
        let dpitch_raw = Rad(self.rotate_vertical) * self.sensitivity * dt;
        if dyaw.0 != 0.0 || dpitch_raw.0 != 0.0 {
            let forward = camera.orientation.rotate_vector(-Vector3::unit_z()).normalize();
            let pitch_now = forward.y.asin();
            let desired = (pitch_now + dpitch_raw.0).clamp(-SAFE_FRAC_PI_2, SAFE_FRAC_PI_2);
            let dpitch_clamped = desired - pitch_now;

            let q_yaw = Quaternion::from_axis_angle(Vector3::unit_y(), dyaw);
            let right = camera.orientation.rotate_vector(Vector3::unit_x()).normalize();
            let q_pitch = Quaternion::from_axis_angle(right, Rad(dpitch_clamped));
            camera.orientation = (q_yaw * q_pitch) * camera.orientation;

            self.rotate_horizontal = 0.0;
            self.rotate_vertical = 0.0;
        }

        let max_distance: f32 = 20_000_000_000.0; // 20 billion km
        let scroll_input = self.scroll; // positive -> zoom out, negative -> zoom in
        let dynamic_scale = (distance * 0.2).max(1_000.0);
        distance = (distance - scroll_input * self.sensitivity * dt * dynamic_scale)
            .clamp(min_distance, max_distance);
        self.scroll = 0.0;
        camera.distance = distance;

        let back = camera.orientation.rotate_vector(Vector3::unit_z()).normalize();
        let new_pos = target_graphics.to_vec() + back * distance;
        camera.position = Point3::from_vec(new_pos);
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    view_position: [f32; 4],
    view: [[f32; 4]; 4],
    view_proj: [[f32; 4]; 4],
    inv_proj: [[f32; 4]; 4],
    inv_view: [[f32; 4]; 4],
}

impl CameraUniform {
    
    pub fn new() -> Self {
        Self {
            view_position: [0.0; 4],
            view: cgmath::Matrix4::identity().into(),
            view_proj: cgmath::Matrix4::identity().into(),
            inv_proj: cgmath::Matrix4::identity().into(), 
            inv_view: cgmath::Matrix4::identity().into(),
        }
    }

    pub fn update_view_proj(&mut self, camera: &Camera, projection: &Projection) {
        self.view_position = camera.position.to_homogeneous().into();
        let proj = projection.calc_matrix();
        let view = camera.calc_matrix();
        let view_proj = proj * view;
        self.view = view.into();
        self.view_proj = view_proj.into();
        self.inv_proj = proj.invert().unwrap().into();
        self.inv_view = view.invert().unwrap().into();
    }
}