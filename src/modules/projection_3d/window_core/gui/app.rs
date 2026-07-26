use std::sync::Arc;

use crate::modules::{
    projection_3d::window_core::gui::{kernel_setup::KernelSetupState, window::AppState},
    utils,
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::PhysicalKey,
    window::{Fullscreen, Window},
};

enum AppScreen {
    /// Shown first.
    KernelSetup(KernelSetupState),
    /// 3D application after Start succeeds.
    Simulation(AppState),
}

pub struct App {
    state: Option<AppScreen>,
    last_time: std::time::Instant,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: None,
            last_time: std::time::Instant::now(),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        #[allow(unused_mut)]
        let mut window_attributes = Window::default_attributes();

        let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
        window.set_title("AXBIT v0.1.0");

        window.set_fullscreen(Some(Fullscreen::Borderless(None)));

        self.state = Some(AppScreen::KernelSetup(
            pollster::block_on(KernelSetupState::new(window)).unwrap(),
        ));
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let Some(AppScreen::Simulation(state)) = &mut self.state {
            match event {
                DeviceEvent::MouseMotion { delta: (dx, dy) } => {
                    if state.right_pressed {
                        state.camera_controller.handle_mouse(dx, dy);
                        state.window.request_redraw();
                    }
                }
                _ => {}
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else { return };
        let egui_response = match state {
            AppScreen::KernelSetup(state) => state.handle_input(&event),
            AppScreen::Simulation(state) => state.egui_renderer.handle_input(&state.window, &event),
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => match state {
                AppScreen::KernelSetup(state) => state.resize(size.width, size.height),
                AppScreen::Simulation(state) => state.resize(size.width, size.height),
            },
            WindowEvent::RedrawRequested => {
                let mut simulation_start_request = None;
                let mut kernel_setup_request = None;

                let render_result = match state {
                    AppScreen::KernelSetup(state) => {
                        let result = state.render();
                        if let Some(selection) = state.start_sim_selection() {
                            simulation_start_request = Some((state.window.clone(), selection));
                        }
                        result
                    }
                    AppScreen::Simulation(state) => {
                        let dt = self.last_time.elapsed();
                        self.last_time = std::time::Instant::now();
                        state.update(dt);
                        let result = state.render();
                        if let Some(manifest) = state.kernel_setup_request() {
                            kernel_setup_request = Some((state.window.clone(), manifest));
                        }
                        result
                    }
                };

                if let Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) = render_result
                {
                    match state {
                        AppScreen::KernelSetup(state) => {
                            let size = state.window.inner_size();
                            state.resize(size.width, size.height);
                        }
                        AppScreen::Simulation(state) => {
                            let size = state.window.inner_size();
                            state.resize(size.width, size.height);
                        }
                    }
                } else if let Err(error) = render_result {
                    log::error!("Unable to render {}", error);
                }

                if let Some((window, selection)) = simulation_start_request {
                    // The same window gets a new surface/device owned by AppState.
                    self.state = None;
                    let manifest = selection.manifest_path.clone();
                    match pollster::block_on(AppState::new(window.clone(), selection)) {
                        Ok(mut state) => {
                            let size = state.window.inner_size();
                            state.resize(size.width, size.height);
                            self.last_time = std::time::Instant::now();
                            self.state = Some(AppScreen::Simulation(state));
                        }
                        Err(error) => {
                            log::error!("Could not start simulation: {}", error);
                            let mut setup = pollster::block_on(
                                KernelSetupState::new_with_manifest(window, manifest),
                            )
                            .unwrap();
                            setup
                                .show_start_error(format!("Could not start simulation: {}", error));
                            self.state = Some(AppScreen::KernelSetup(setup));
                        }
                    }
                } else if let Some((window, manifest)) = kernel_setup_request {
                    utils::clear_spice_m();
                    self.state = None;
                    let setup = pollster::block_on(KernelSetupState::new_with_manifest(
                        window, manifest,
                    ))
                    .unwrap();
                    self.state = Some(AppScreen::KernelSetup(setup));
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                // Only forward mouse clicks to the 3D scene if egui
                // didn't consume the event (e.g. when clicking in a UI).
                if !egui_response.consumed {
                    let AppScreen::Simulation(state) = state else {
                        return;
                    };
                    state.handle_mouse_button(button, btn_state.is_pressed());
                }
                match state {
                    AppScreen::KernelSetup(state) => state.window.request_redraw(),
                    AppScreen::Simulation(state) => state.window.request_redraw(),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Avoid zooming the camera when scrolling inside egui
                // widgets such as the date picker or settings window.
                if !egui_response.consumed {
                    let AppScreen::Simulation(state) = state else {
                        return;
                    };
                    state.handle_mouse_scroll(&delta);
                }
                match state {
                    AppScreen::KernelSetup(state) => state.window.request_redraw(),
                    AppScreen::Simulation(state) => state.window.request_redraw(),
                }
            }
            WindowEvent::CursorMoved {
                device_id: _,
                position,
            } => {
                // Store cursor position in the same coordinate space
                // as the window's size used for configuring the surface
                // and picking texture.
                match state {
                    AppScreen::KernelSetup(state) => state.window.request_redraw(),
                    AppScreen::Simulation(state) => {
                        state.mouse_x = position.x;
                        state.mouse_y = position.y;
                        state.window.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        ..
                    },
                ..
            } => {
                // Don't move the camera or control the sim when typing
                // into egui widgets (text fields, shortcuts, etc.).
                if !egui_response.consumed {
                    let AppScreen::Simulation(state) = state else {
                        return;
                    };
                    state.handle_key(event_loop, code, key_state.is_pressed());
                }
                match state {
                    AppScreen::KernelSetup(state) => state.window.request_redraw(),
                    AppScreen::Simulation(state) => state.window.request_redraw(),
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        match &mut self.state {
            Some(AppScreen::KernelSetup(state)) => state.window.request_redraw(),
            Some(AppScreen::Simulation(state)) => {
                let needs_continuous_redraw = !state.simulation.paused
                    || state.show_integrator_window
                    || state.show_settings
                    || state.show_sb_manager
                    || state.right_pressed;
                if needs_continuous_redraw {
                    state.window.request_redraw();
                }
            }
            None => {}
        }
    }
}

pub fn run() -> anyhow::Result<()> {
    //env_logger::init();
    let event_loop = EventLoop::new()?;
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    utils::clear_spice_m();
    Ok(())
}
