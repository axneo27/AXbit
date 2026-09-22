mod celestial_info;
mod common;
mod integrator;
mod sb_manager;
mod settings;
mod top_bar;

use super::window::AppState;

pub use top_bar::SPEEDS;

pub fn update_gui(state: &mut AppState) {
    if !state.settings_loaded {
        settings::load_settings(state);
        state.settings_loaded = true;
        // Persist the choices made on the startup kernel screen.
        state.settings_dirty = !state.groups_to_load.is_empty();
    }

    state.egui_renderer.begin_frame(&state.window);
    // Clone the egui Context so we don't keep an outstanding
    // borrow of `state` while building UI that also mutates it.
    let ctx = state.egui_renderer.context().clone();

    ctx.style_mut(|style| {
        style.visuals.window_shadow = egui::epaint::Shadow::NONE;
    });

    top_bar::show(state, &ctx);
    settings::show(state, &ctx);
    sb_manager::show(state, &ctx);
    integrator::show(state, &ctx);
    celestial_info::show(state, &ctx);
    settings::save_if_dirty(state);
}
