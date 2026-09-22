use super::super::window::AppState;
use super::common::rebuild_instances_and_gpu;
use crate::modules::projection_3d::traj::ProximityReference;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct SavedSettings {
    show_barycenters: bool,
    show_small_bodies: bool,
    sb_filter: Vec<i32>,
    set_year: i32,
    set_month: u32,
    set_day: u32,
    set_hour: u32,
    set_minute: u32,
    set_second: u32,
    #[serde(default)]
    integrator_proximity_reference: ProximityReference,
    #[serde(default)]
    integrator_proximity_multiplier: u32,
    #[serde(default)]
    loaded_groups: Vec<String>,
    #[serde(default)]
    kernel_file_selections: std::collections::HashMap<String, Vec<usize>>,
}

fn settings_path() -> &'static std::path::Path {
    crate::modules::app_paths::get().settings()
}

pub(super) fn load_settings(state: &mut AppState) {
    let path = settings_path();

    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(saved) = toml::from_str::<SavedSettings>(&data) {
            state.simulation.show_barycenters = saved.show_barycenters;
            state.simulation.show_small_bodies = saved.show_small_bodies;

            state.simulation.sb_filter = saved.sb_filter.iter().copied().collect();
            state.simulation.sb_orbit_filter = state.simulation.sb_filter.clone();

            state.set_year = saved.set_year;
            state.set_month = saved.set_month;
            state.set_day = saved.set_day;
            state.set_hour = saved.set_hour;
            state.set_minute = saved.set_minute;
            state.set_second = saved.set_second;
            state.integrator_proximity_reference = saved.integrator_proximity_reference;
            state.integrator_proximity_multiplier = saved.integrator_proximity_multiplier;

            rebuild_instances_and_gpu(state);
        }
    }
}

fn save_settings(state: &AppState) {
    let mut sb_ids: Vec<i32> = state.simulation.sb_filter.iter().copied().collect();
    sb_ids.sort();

    let saved = SavedSettings {
        show_barycenters: state.simulation.show_barycenters,
        show_small_bodies: state.simulation.show_small_bodies,
        sb_filter: sb_ids,
        set_year: state.set_year,
        set_month: state.set_month,
        set_day: state.set_day,
        set_hour: state.set_hour,
        set_minute: state.set_minute,
        set_second: state.set_second,
        integrator_proximity_reference: state.integrator_proximity_reference,
        integrator_proximity_multiplier: state.integrator_proximity_multiplier,
        loaded_groups: state.groups_to_load.clone(),
        kernel_file_selections: state.advanced_kernel_selections.clone(),
    };

    if let Ok(toml) = toml::to_string_pretty(&saved) {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, toml);
    }
}

pub(super) fn show(state: &mut AppState, ctx: &egui::Context) {
    if state.show_settings {
        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(false)
            .open(&mut state.show_settings)
            .show(&ctx, |ui| {
                ui.label("FPS Limit:");
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(state.fps_limit == Some(30.0), "30")
                        .clicked()
                    {
                        state.fps_limit = Some(30.0);
                    }
                    if ui
                        .selectable_label(state.fps_limit == Some(60.0), "60")
                        .clicked()
                    {
                        state.fps_limit = Some(60.0);
                    }
                    if ui
                        .selectable_label(state.fps_limit == Some(120.0), "120")
                        .clicked()
                    {
                        state.fps_limit = Some(120.0);
                    }
                });
                ui.separator();
                let mut settings_changed = false;
                settings_changed |= ui
                    .checkbox(&mut state.simulation.show_barycenters, "Show Barycenters")
                    .changed();
                settings_changed |= ui
                    .checkbox(&mut state.simulation.show_small_bodies, "Show SBDB objects")
                    .changed();

                ui.separator();
                ui.label("Close-approach threshold for SB integration:");
                ui.horizontal(|ui| {
                    settings_changed |= ui
                        .radio_value(
                            &mut state.integrator_proximity_reference,
                            ProximityReference::HillSphere,
                            "Hill sphere",
                        )
                        .changed();
                    settings_changed |= ui
                        .radio_value(
                            &mut state.integrator_proximity_reference,
                            ProximityReference::Soi,
                            "SOI",
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Multiplier:");
                    if ui
                        .selectable_label(state.integrator_proximity_multiplier == 1, "1x")
                        .clicked()
                    {
                        state.integrator_proximity_multiplier = 1;
                        settings_changed = true;
                    }
                    if ui
                        .selectable_label(state.integrator_proximity_multiplier == 2, "2x")
                        .clicked()
                    {
                        state.integrator_proximity_multiplier = 2;
                        settings_changed = true;
                    }
                    if ui
                        .selectable_label(state.integrator_proximity_multiplier == 3, "3x")
                        .clicked()
                    {
                        state.integrator_proximity_multiplier = 3;
                        settings_changed = true;
                    }
                });

                if settings_changed {
                    state.settings_dirty = true;
                }
            });
    }
}

pub(super) fn save_if_dirty(state: &mut AppState) {
    if state.settings_dirty {
        save_settings(state);
        state.settings_dirty = false;
    }
}
