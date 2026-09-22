use super::super::window::AppState;
use super::common::rebuild_instances_and_gpu;
use crate::modules::utils;
use chrono::{Datelike, NaiveDate};
use egui_extras::DatePickerButton;

pub const SPEEDS: &[(f64, &str)] = &[
    (1.0, "1x"),
    (10.0, "10x"),
    (100.0, "100x"),
    (1000.0, "1000x"),
    (10000.0, "10000x"),
    (100000.0, "100000x"),
    (1000000.0, "1000000x"),
];

pub(super) fn show(state: &mut AppState, ctx: &egui::Context) {
    egui::TopBottomPanel::top("menu").show(&ctx, |ui| {
        let integration_mode_active = state.show_integrator_window || state.integrator_in_progress;
        ui.horizontal(|ui| {
            if ui.button("SPICE Kernels Setup").clicked() {
                state.return_to_kernel_setup = true;
            }
            ui.label("FPS:");
            ui.label(format!("{:.1}", state.fps_display));
            ui.separator();
            if ui.button("Settings").clicked() {
                state.show_settings = !state.show_settings;
            }
            if ui.button("SBDB").clicked() {
                state.show_sb_manager = !state.show_sb_manager;
            }
            ui.separator();
            ui.label(utils::format_utc_display(
                state.simulation.current_time_utc(),
            ));
            ui.separator();
            ui.label("Speed:");

            let mut current_label = String::from("custom");
            for (v, label) in SPEEDS.iter() {
                if (*v - state.simulation.time_speed()).abs() < f64::EPSILON {
                    current_label = label.to_string();
                    break;
                }
            }

            egui::ComboBox::from_id_salt("sim_speed_combo")
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    for (v, label) in SPEEDS.iter() {
                        if ui
                            .selectable_label(
                                (*v - state.simulation.time_speed()).abs() < f64::EPSILON,
                                *label,
                            )
                            .clicked()
                        {
                            state.simulation.set_time_speed(*v);
                            state.now_button_highlighted = false;
                            state.settings_dirty = true;
                        }
                    }
                });

            if !integration_mode_active {
                ui.separator();
                ui.label("Set Time:");

                let mut date =
                    NaiveDate::from_ymd_opt(state.set_year, state.set_month, state.set_day)
                        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2000, 1, 1).unwrap());

                let (year_min, year_max) = state.simulation.time_bounds_years();
                if ui
                    .add(DatePickerButton::new(&mut date).start_end_years(year_min..=year_max))
                    .changed()
                {
                    state.set_year = date.year();
                    state.set_month = date.month();
                    state.set_day = date.day();
                    state.now_button_highlighted = false;
                    state.settings_dirty = true;
                }
                if ui
                    .add(
                        egui::DragValue::new(&mut state.set_hour)
                            .range(0..=23)
                            .prefix("H:"),
                    )
                    .changed()
                {
                    state.now_button_highlighted = false;
                }
                if ui
                    .add(
                        egui::DragValue::new(&mut state.set_minute)
                            .range(0..=59)
                            .prefix("M:"),
                    )
                    .changed()
                {
                    state.now_button_highlighted = false;
                }
                if ui
                    .add(
                        egui::DragValue::new(&mut state.set_second)
                            .range(0..=59)
                            .prefix("S:"),
                    )
                    .changed()
                {
                    state.now_button_highlighted = false;
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    state.settings_dirty = true;
                }
                if ui.button("Set").clicked() {
                    state.now_button_highlighted = false;
                    let utc = format!(
                        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                        state.set_year,
                        state.set_month,
                        state.set_day,
                        state.set_hour,
                        state.set_minute,
                        state.set_second
                    );
                    match state.simulation.set_time(&utc) {
                        Ok(_) => {
                            state.time_error = None;
                            rebuild_instances_and_gpu(state);
                        }
                        Err(e) => state.time_error = Some(e),
                    }
                }
                let now_btn = if state.now_button_highlighted {
                    ui.button(egui::RichText::new("Now").color(egui::Color32::GREEN))
                } else {
                    ui.button("Now")
                };
                if now_btn.clicked() {
                    if let Some(utc) = utils::current_utc() {
                        match state.simulation.set_time(&utc) {
                            Ok(_) => {
                                state.time_error = None;
                                if !state.simulation.paused {
                                    state.now_button_highlighted = true;
                                }
                                rebuild_instances_and_gpu(state);
                            }
                            Err(e) => state.time_error = Some(e),
                        }
                    }
                }
                if let Some(ref err) = state.time_error {
                    ui.label(egui::RichText::new(err).color(egui::Color32::RED));
                }
                ui.separator();
            }
            if integration_mode_active {
                ui.label("Integration mode");
            } else if state.simulation.paused {
                if ui.button("Resume").clicked() {
                    state.simulation.paused = false;
                }
            } else if ui.button("Pause").clicked() {
                state.simulation.paused = true;
                state.now_button_highlighted = false; /////
            }
        });
    });
}
