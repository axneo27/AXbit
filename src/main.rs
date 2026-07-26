pub mod modules {
    pub mod projection_3d {
        pub mod simulation;
        pub mod celestial;
        pub mod state;
        pub mod traj;
        pub mod pipelines {
            pub mod common;
            pub mod indirect;
            pub mod orbit;
            pub mod celestial_marker;
            pub mod closest_approach;
            pub mod body;
            pub mod trajectory;
        }
        pub mod window_core {
            pub mod gui {
                pub mod egui_tools;
                pub mod kernel_setup;
                pub mod window;
                pub mod ui;
                pub mod app;
            }
            pub mod camera;
            pub mod texture;
            pub mod instance;
            pub mod model;
            pub mod vertex;
            pub mod resources;
            pub mod light;
            pub mod hdr;
        }
    }
    pub mod tests;
    pub mod utils;
    pub mod app_paths;
    pub mod sbdb;
    pub mod kernel_config;
    pub mod kernel_manager;
    pub mod spice_bindings;
    pub mod spice_ker;
}
use modules::projection_3d::window_core::gui::app::run;
use dotenv::dotenv;

fn main() {
    dotenv().ok();

    env_logger::init();
    if let Err(e) = modules::app_paths::init() {
        eprintln!("Failed to initialize application paths: {e:#}");
        return;
    }
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
    }
}
