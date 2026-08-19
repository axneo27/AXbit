use super::window::{AppState, SbManagerTab};
use cgmath::InnerSpace;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Duration};
use egui_extras::DatePickerButton;
use crate::modules::{projection_3d::state::StateVector, spice_ker, utils::{
    YMDHMS, DistanceUnit, self}};
use crate::modules::sbdb;
use crate::modules::projection_3d::simulation::FocusedBodyType;
use std::sync::mpsc::TryRecvError;
use std::sync::mpsc;
use serde::{Deserialize, Serialize};
use toml;
use crate::modules::projection_3d::traj::{Integrator, ProximityReference};
use std::format;

pub const SPEEDS: &[(f64, &str)] = &[
    (1.0, "1x"),
    (10.0, "10x"),
    (100.0, "100x"),
    (1000.0, "1000x"),
    (10000.0, "10000x"),
    (100000.0, "100000x"),
    (1000000.0, "1000000x"),
];

const SECONDS_PER_DAY: f64 = 86400.0;
const CLOSE_APPROACH_DAYS_BEFORE: i64 = 75;
const CLOSE_APPROACH_DAYS_AFTER: i64 = 7;
const DRAGVALUE_BASE_SPEED_AU: f64 = 0.00001;

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

fn load_settings(state: &mut AppState) {
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

fn format_with_commas(val: f64, decimals: usize) -> String {
    let formatted = format!("{:.decimals$}", val);
    let parts: Vec<&str> = formatted.split('.').collect();
    let int_part = parts[0];
    let frac_part = parts.get(1).copied().unwrap_or("");
    let chars: Vec<char> = int_part.chars().rev().collect();
    let mut result = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    let formatted_int = result.chars().rev().collect::<String>();
    if frac_part.is_empty() {
        formatted_int
    } else {
        format!("{}.{}", formatted_int, frac_part)
    }
}

fn rebuild_instances_and_gpu(state: &mut AppState) {
    state.simulation.initialize_instances();
    state.render_origin = state.simulation.render_origin();
    state.simulation.upd_ins_pos_ro(state.render_origin);
    state.celestial_marker_pipeline.update_instances(
        &state.device,
        &state.queue,
        &state.simulation.celestial_marker_instances,
        false,
        true,
    );
    state.celestial_marker_pipeline.update_visibility(
        &state.queue,
        &state.simulation.visible_ids(),
        &state.simulation.celestial_marker_instances,
    );
    state.orbit_pipeline.update_instances(
        &state.device,
        &state.queue,
        &state.simulation.orbit_instances,
        false,
        true,
    );
    state.orbit_pipeline.update_visibility(
        &state.queue,
        &state.simulation.orbit_visible_ids(),
        &state.simulation.orbit_instances,
    );
    state.body_pipeline.update_instances(
        &state.device,
        &state.queue,
        &state.simulation.body_instances,
        false,
        true,
    );
    state.body_pipeline.update_visibility(
        &state.queue,
        &state.simulation.visible_ids(),
        &state.simulation.body_instances,
    );
}

fn sync_sim_time_to_integrator_start(state: &mut AppState) {
    let utc = state.integrator_start.utc_string();
    if state.simulation.set_time(&utc).is_ok() {
        rebuild_instances_and_gpu(state);
    }
}

fn process_sb_download(state: &mut AppState) {
    if !state.sb_download_in_progress {
        return;
    }
    if let Some(rx) = &state.sb_download_result_rx {
        match rx.try_recv() {
            Ok(result) => {
                state.sb_download_in_progress = false;
                state.sb_download_result_rx = None;
                match (result, state.sb_current_download_id.take()) {
                    (Ok(id), Some(_)) => match state.simulation.add_small_body(id) {
                        Ok(_) => {
                            rebuild_instances_and_gpu(state);
                            state.sb_status = Some(format!("Loaded SB {} into simulation", id));
                        }
                        Err(e) => state.sb_status = Some(format!("Failed to add SB to simulation: {}", e)),
                    },
                    (Err(e), Some(search)) => {
                        state.sb_status = Some(format!("Download failed for {}: {}", search, e));
                    }
                    (Ok(_), None) => {
                        state.sb_status = Some("Download finished, but no ID recorded".to_string());
                    }
                    (Err(e), None) => state.sb_status = Some(format!("Download failed: {}", e)),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                state.sb_download_in_progress = false;
                state.sb_download_result_rx = None;
                state.sb_status = Some("Download channel disconnected".to_string());
            }
        }
    }
}

fn start_sb_download(state: &mut AppState, designation: &str, name: &str) {
    let search = designation.to_string();
    let (tx, rx) = mpsc::channel::<Result<i32, String>>();

    state.sb_download_in_progress = true;
    state.sb_download_result_rx = Some(rx);
    state.sb_current_download_id = Some(search.clone());
    state.sb_status = Some(format!("Downloading {}...", name));

    std::thread::spawn(move || {
        let result = sbdb::download_and_store_small_body(&search)
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
}

fn prepare_close_approach_integration(
    state: &mut AppState,
    small_body_id: i32,
    close_approach: &sbdb::CloseApproachData,
) -> Result<(), String> {
    let tca = utils::jd_to_et(close_approach.tca_jd)
        .map_err(|e| e.to_string())?;

    // Give the integrator time to propagate toward the encounter, then keep a
    // short period after TCA so the complete flyby is visible.
    let start_et = tca - CLOSE_APPROACH_DAYS_BEFORE as f64 * SECONDS_PER_DAY;
    let start_utc = utils::et_to_utc(start_et)
        .map_err(|e| e.to_string())?;

    let truncated = &start_utc[..std::cmp::min(start_utc.len(), 19)];
    let start = NaiveDateTime::parse_from_str(truncated, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| e.to_string())?;
    let integration_days = CLOSE_APPROACH_DAYS_BEFORE + CLOSE_APPROACH_DAYS_AFTER;

    state.integrator_start = YMDHMS::from_datetime(start);
    state.integrator_end = YMDHMS::from_datetime(start + Duration::days(integration_days));
    state.integrator_sb_id = Some(small_body_id);

    state.simulation.focused_body_type = FocusedBodyType::SmallBody;
    state.simulation.focused_body_id = small_body_id;
    state.simulation.paused = true;
    state.now_button_highlighted = false;
    state.show_integrator_window = true;

    Ok(())
}

pub fn update_gui(state: &mut AppState) {
    if !state.settings_loaded {
        load_settings(state);
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
            ui.label(utils::format_utc_display(state.simulation.current_time_utc()));
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
                        if ui.selectable_label((*v - state.simulation.time_speed()).abs() < f64::EPSILON, *label).clicked() {
                            state.simulation.set_time_speed(*v);
                            state.now_button_highlighted = false;
                            state.settings_dirty = true;
                        }
                    }
                });

            if !integration_mode_active {
                ui.separator();
                ui.label("Set Time:");
                
                let mut date = NaiveDate::from_ymd_opt(
                    state.set_year,
                    state.set_month,
                    state.set_day,
                )
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
                if ui.add(
                    egui::DragValue::new(&mut state.set_hour)
                        .range(0..=23)
                        .prefix("H:"),
                ).changed() {
                    state.now_button_highlighted = false;
                }
                if ui.add(
                    egui::DragValue::new(&mut state.set_minute)
                        .range(0..=59)
                        .prefix("M:"),
                ).changed() {
                    state.now_button_highlighted = false;
                }
                if ui.add(
                    egui::DragValue::new(&mut state.set_second)
                        .range(0..=59)
                        .prefix("S:"),
                ).changed() {
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
                    .checkbox(
                        &mut state.simulation.show_barycenters,
                        "Show Barycenters",
                    )
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

    if state.show_sb_manager {
        process_sb_download(state);
        let mut open = state.show_sb_manager;
        egui::Window::new("SBDB Manager")
            .open(&mut open)
            .vscroll(true)
            .show(&ctx, |ui| {
                if state.integrator_in_progress {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "SBDB operations disabled during integration.",
                    );
                    return;
                }
                ui.hyperlink_to(
                    "SBDB Lookup (JPL SBDB)",
                    "https://ssd.jpl.nasa.gov/tools/sbdb_query.html",
                );
                ui.hyperlink_to(
                    "NEO Earth Close Approaches",
                    "https://cneos.jpl.nasa.gov/ca/"
                );
                ui.separator();

                ui.horizontal(|ui| {
                    ui.selectable_value(&mut state.sb_manager_tab, SbManagerTab::Search, "Objects");
                    ui.selectable_value(&mut state.sb_manager_tab, SbManagerTab::CloseApproaches, "Close approaches");
                });
                ui.separator();

                if state.sb_manager_tab == SbManagerTab::CloseApproaches {
                    let planets_ids = if state.sb_cad_loaded_planets_ids.is_empty() {
                        state.sb_cad_loaded_planets_ids = spice_ker::get_all_planet_ids()
                            .unwrap_or(spice_ker::STANDARD_PLANET_IDS.to_vec());
                        state.sb_cad_loaded_planets_ids.clone()
                    } else {
                        state.sb_cad_loaded_planets_ids.clone()
                    };
                    let mut selected_planet = state.sb_cad_filters.encounter_body
                        .clone()
                        .unwrap_or("All".to_owned());

                    ui.horizontal(|ui| {
                        ui.label("Encounter body:");
                        egui::ComboBox::from_id_salt("close_approach_planet_combo")
                            .selected_text(&selected_planet)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut selected_planet, "All".to_owned(), "All");
                                for id in planets_ids {
                                    if let Some(planet_name) = spice_ker::planet_id_to_name(&id) {
                                        ui.selectable_value(
                                            &mut selected_planet,
                                            planet_name.to_owned(),
                                            planet_name,
                                        );
                                    }
                                }
                            });
                    });

                    state.sb_cad_filters.encounter_body = if selected_planet == "All" {
                        None
                    } else {
                        sbdb::planet_name_to_cad_name(&selected_planet).map(str::to_string)
                    };

                    ui.horizontal(|ui| {
                        ui.label("From:");
                        let mut start_date = state.sb_cad_filters.start_date.date()
                            .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
                        if ui.add(DatePickerButton::new(&mut start_date)).changed() {
                            state.sb_cad_filters.start_date.year = start_date.year();
                            state.sb_cad_filters.start_date.month = start_date.month();
                            state.sb_cad_filters.start_date.day = start_date.day();
                        }

                        ui.label("To:");
                        let mut end_date = state.sb_cad_filters.end_date.date()
                            .unwrap_or_else(|| NaiveDate::from_ymd_opt(2031, 1, 1).unwrap());
                        if ui.add(DatePickerButton::new(&mut end_date)).changed() {
                            state.sb_cad_filters.end_date.year = end_date.year();
                            state.sb_cad_filters.end_date.month = end_date.month();
                            state.sb_cad_filters.end_date.day = end_date.day();
                        }
                    });

                    ui.horizontal(|ui| {
                        let max_dist = match state.sb_cad_unit {
                            DistanceUnit::AU => state.sb_cad_filters.maximum_distance_au,
                            DistanceUnit::KM => utils::au_to_km(state.sb_cad_filters.maximum_distance_au),
                        };

                        let range = match state.sb_cad_unit {
                            DistanceUnit::AU => 0.0..=1.0,
                            DistanceUnit::KM => 0.0..=149_597_870.7,
                        };

                        let speed = match state.sb_cad_unit {
                            DistanceUnit::AU => DRAGVALUE_BASE_SPEED_AU,
                            DistanceUnit::KM => utils::au_to_km(DRAGVALUE_BASE_SPEED_AU)
                        };

                        let mut new_max_dist = max_dist;

                        ui.label("Maximum distance:");

                        if ui.add(
                            egui::DragValue::new(&mut new_max_dist)
                                .range(range)
                                .speed(speed)
                                .suffix(state.sb_cad_unit.to_string())
                        ).changed() {

                            state.sb_cad_filters.maximum_distance_au = match state.sb_cad_unit {
                                DistanceUnit::AU => new_max_dist,
                                DistanceUnit::KM => utils::km_to_au(new_max_dist),
                            };
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut state.sb_cad_unit, DistanceUnit::AU, "AU");
                        ui.selectable_value(&mut state.sb_cad_unit, DistanceUnit::KM, "km");
                    });

                    ui.checkbox(&mut state.sb_cad_filters.downloaded_only, "Downloaded objects only");

                    let search_in_progress = state.sb_cad_results_rx.is_some();
                    if ui.add_enabled(!search_in_progress, egui::Button::new("Find close approaches")).clicked() {
                        if state.sb_cad_filters.end_date.date() < state.sb_cad_filters.start_date.date() {
                            state.sb_cad_search_status = Some("End date must be after start date".to_string());
                        } else {
                            let filters = state.sb_cad_filters.clone();
                            let (tx, rx) = mpsc::channel::<Result<Vec<sbdb::CloseApproachData>, String>>();
                            state.sb_cad_results_rx = Some(rx);
                            state.sb_cad_search_status = Some("Searching close approaches...".to_string());
                            std::thread::spawn(move || {
                                let result = sbdb::search_cad_objects(&filters).map_err(|e| e.to_string());
                                let _ = tx.send(result);
                            });
                        }
                    }

                    if let Some(rx) = &state.sb_cad_results_rx {
                        match rx.try_recv() {
                            Ok(result) => {
                                state.sb_cad_results_rx = None;
                                match result {
                                    Ok(results) => {
                                        state.sb_cad_search_status = Some(format!("Found {} close approach(es)", results.len()));
                                        state.sb_cad_results = results;
                                    }
                                    Err(e) => state.sb_cad_search_status = Some(format!("Search failed: {}", e)),
                                }
                            }
                            Err(TryRecvError::Empty) => {}
                            Err(TryRecvError::Disconnected) => {
                                state.sb_cad_results_rx = None;
                                state.sb_cad_search_status = Some("Search channel disconnected".to_string());
                            }
                        }
                    }

                    if state.sb_cad_results_rx.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Searching JPL close approaches...");
                        });
                    }
                    if let Some(msg) = &state.sb_cad_search_status {
                        ui.label(msg);
                    }
                    if state.sb_download_in_progress {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(state.sb_status.as_deref().unwrap_or("Downloading SBDB data..."));
                        });
                    } else if let Some(msg) = &state.sb_status {
                        ui.label(msg);
                    }

                    let downloaded = sbdb::list_downloaded_bodies().unwrap_or_default();
                    let results = state.sb_cad_results.clone();
                    for result in results {
                        let downloaded_body = downloaded.iter().find(|body| body.des == result.designation);
                        if state.sb_cad_filters.downloaded_only && downloaded_body.is_none() {
                            continue;
                        }

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                let nominal_distance = match state.sb_cad_unit {
                                    DistanceUnit::AU => result.nominal_distance_au,
                                    DistanceUnit::KM => utils::au_to_km(result.nominal_distance_au),
                                };
                                let distance_decimals = match state.sb_cad_unit {
                                    DistanceUnit::AU => 6,
                                    DistanceUnit::KM => 0,
                                };

                                ui.strong(&result.sb_name);
                                ui.label(format!("{} · {}", result.encounter_body, result.tca_calendar));
                                ui.small(format!(
                                    "Distance: {} {} · Relative velocity: {:.3} km/s",
                                    format_with_commas(nominal_distance, distance_decimals),
                                    state.sb_cad_unit.to_string(),
                                    result.relative_velocity_km_s,
                                ));

                                if let (Some(min), Some(max)) = (
                                    result.minimum_3sigma_distance_au,
                                    result.maximum_3sigma_distance_au,
                                ) {
                                    let (min, max) = match state.sb_cad_unit {
                                        DistanceUnit::AU => (min, max),
                                        DistanceUnit::KM => (utils::au_to_km(min), utils::au_to_km(max)),
                                    };

                                    ui.small(format!(
                                        "3σ distance: {}–{} {}",
                                        format_with_commas(min, distance_decimals),
                                        format_with_commas(max, distance_decimals),
                                        state.sb_cad_unit.to_string(),
                                    ));
                                }

                                if let Some(uncertainty) = &result.time_uncertainty {
                                    ui.small(format!("3σ time uncertainty: {}", uncertainty));
                                }
                            });

                            if let Some(body) = downloaded_body {
                                let prepare = ui.button("Prepare integration").on_hover_text(format!(
                                    "Set the integration window from {} days before to {} days after closest approach",
                                    CLOSE_APPROACH_DAYS_BEFORE,
                                    CLOSE_APPROACH_DAYS_AFTER,
                                ));

                                if prepare.clicked() {
                                    match prepare_close_approach_integration(state, body.id, &result) {
                                        Ok(()) => {
                                            state.sb_cad_search_status = Some(format!(
                                                "Integration prepared {} days before close approach",
                                                CLOSE_APPROACH_DAYS_BEFORE,
                                            ));
                                        }
                                        Err(e) => {
                                            state.sb_cad_search_status = Some(format!(
                                                "Could not prepare integration: {}",
                                                e,
                                            ));
                                        }
                                    }
                                }
                            } else if ui
                                .add_enabled(
                                    !state.sb_download_in_progress,
                                    egui::Button::new("Download"),
                                )
                                .clicked()
                            {
                                start_sb_download(state, &result.designation, &result.sb_name);
                            }
                        });
                    }
                }

                if state.sb_manager_tab == SbManagerTab::Search {
                    ui.horizontal(|ui| {
                        ui.label("Name, designation or ID:");
                        let response = ui.text_edit_singleline(&mut state.sb_download_id_input);
                        let search_clicked = ui.button("Search").clicked()
                            || response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));

                        if search_clicked && !state.sb_search_in_progress {
                            let search = state.sb_download_id_input.trim().to_string();
                            if search.is_empty() {
                                state.sb_search_results.clear();
                                state.sb_status = None;
                            } else {
                                let (tx, rx) = mpsc::channel::<Result<Vec<sbdb::SbdbSearchResult>, String>>();
                                state.sb_search_in_progress = true;
                                state.sb_search_result_rx = Some(rx);
                                state.sb_status = Some(format!("Searching SBDB for '{}'...", search));

                                std::thread::spawn(move || {
                                    let result = sbdb::search_sbdb_objects(&search)
                                        .map_err(|e| e.to_string());
                                    let _ = tx.send(result);
                                });
                            }
                        }
                    });

                    if state.sb_download_id_input.trim().is_empty() {
                        state.sb_search_results.clear();
                    }

                    if state.sb_search_in_progress {
                        if let Some(rx) = &state.sb_search_result_rx {
                            match rx.try_recv() {
                                Ok(result) => {
                                    state.sb_search_in_progress = false;
                                    state.sb_search_result_rx = None;
                                    match result {
                                        Ok(results) => {
                                            state.sb_status = Some(format!("Found {} SBDB object(s)", results.len()));
                                            state.sb_search_results = results;
                                        }
                                        Err(e) => state.sb_status = Some(format!("Search failed: {}", e)),
                                    }
                                }
                                Err(TryRecvError::Empty) => {}
                                Err(TryRecvError::Disconnected) => {
                                    state.sb_search_in_progress = false;
                                    state.sb_search_result_rx = None;
                                    state.sb_status = Some("Search channel disconnected".to_string());
                                }
                            }
                        }
                    }

                    if state.sb_search_in_progress {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Searching SBDB...");
                        });
                    }

                    if !state.sb_download_id_input.trim().is_empty() {
                        let downloaded = sbdb::list_downloaded_bodies().unwrap_or_default();
                        let results = state.sb_search_results.clone();
                        for result in results {
                            let is_downloaded = downloaded.iter().any(|body| {
                                body.spk_id == result.id || body.des == result.designation
                            });

                            if is_downloaded {
                                continue;
                            }

                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(format!("{} ({})", result.name, result.id));
                                    if let Some(description) = &result.description {
                                        ui.small(description);
                                    }
                                });

                                if ui.add_enabled(!state.sb_download_in_progress, egui::Button::new("Download")).clicked() {
                                    start_sb_download(state, &result.designation, &result.name);
                                }
                            });
                        }
                    }

                    if state.sb_download_in_progress {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Downloading SBDB data...");
                        });
                    }

                    if let Some(msg) = &state.sb_status {
                        ui.label(msg);
                    }

                    ui.separator();
                    ui.heading("Downloaded SBs");

                    ui.horizontal(|ui| {
                        if ui.button("Select all").clicked() {
                            let all: std::collections::HashSet<i32> = state
                                .simulation
                                .sb_celestial_objects
                                .keys()
                                .copied()
                                .collect();
                            state.simulation.sb_orbit_filter = all.clone();
                            state.simulation.sb_filter = all;
                            state.settings_dirty = true;
                        }
                        if ui.button("Deselect all").clicked() {
                            state.simulation.sb_filter.clear();
                            state.simulation.sb_orbit_filter.clear();
                            state.settings_dirty = true;
                        }
                        if ui.button("Delete selected").clicked() {
                            let ids: Vec<i32> = state
                                .simulation
                                .sb_filter
                                .iter()
                                .copied()
                                .collect();
                            if ids.is_empty() {
                                state.sb_status = Some(
                                    "No SBs selected to delete".to_string(),
                                );
                            } else {
                                state.sb_pending_delete = Some(ids);
                            }
                        }
                    });

                    if let Some(ids) = state.sb_pending_delete.clone() {
                        ui.separator();
                        ui.colored_label(
                            egui::Color32::RED,
                            format!(
                                "Are you sure you want to delete {} selected SB(s)?",
                                ids.len()
                            ),
                        );
                        ui.label("This will remove them from the simulation and the local cache.");

                        ui.horizontal(|ui| {
                            if ui.button("Submit").clicked() {
                                state.sb_pending_delete = None;

                                state.simulation.remove_small_bodies(&ids);

                                match sbdb::delete_downloaded_small_bodies(&ids) {
                                    Ok(removed) => {
                                        rebuild_instances_and_gpu(state);
                                        state.sb_status = Some(format!(
                                            "Deleted {} SB(s) ({} removed from cache)",
                                            ids.len(),
                                            removed
                                        ));
                                    }
                                    Err(e) => {
                                        rebuild_instances_and_gpu(state);
                                        state.sb_status = Some(format!(
                                            "Deleted from simulation, but failed to update cache: {}",
                                            e
                                        ));
                                    }
                                }
                            }

                            if ui.button("Cancel").clicked() {
                                state.sb_pending_delete = None;
                                state.sb_status = Some(
                                    "Deletion cancelled".to_string(),
                                );
                            }
                        });
                    }

                    ui.separator();

                    let search = state.sb_download_id_input.trim().to_lowercase();
                    let downloaded = sbdb::list_downloaded_bodies().unwrap_or_default();
                    let sb_ids: Vec<i32> = state.simulation.sb_sorted_ids.to_vec();
                    for id in sb_ids {
                        if let Some(sb) = state.simulation.sb_celestial_objects.get(&id) {
                            let info = downloaded.iter().find(|body| body.id == id);
                            let matches_search = search.is_empty()
                                || id.to_string().contains(&search)
                                || sb.name.to_lowercase().contains(&search)
                                || info.is_some_and(|body| {
                                    body.spk_id.to_lowercase().contains(&search)
                                        || body.des.to_lowercase().contains(&search)
                                        || body.fullname.to_lowercase().contains(&search)
                                });

                            if !matches_search {
                                continue;
                            }

                            ui.horizontal(|ui| {
                                let mut visible = state.simulation.sb_filter.contains(&id);
                                if ui.checkbox(&mut visible, "").changed() {
                                    if visible {
                                        state.simulation.sb_filter.insert(id);
                                        state.simulation.sb_orbit_filter.insert(id);
                                    } else {
                                        state.simulation.sb_filter.remove(&id);
                                        state.simulation.sb_orbit_filter.remove(&id);
                                    }
                                    state.settings_dirty = true;
                                }
                                ui.vertical(|ui| {
                                    ui.label(format!("{} ({})", sb.name, id));
                                    if let Some(info) = info {
                                        if !info.orbit_class_name.is_empty() {
                                            ui.small(&info.orbit_class_name);
                                        }
                                    }
                                });
                                ui.label(egui::RichText::new("Downloaded").color(egui::Color32::GREEN));
                                if ui.button("Focus").clicked() {
                                    state.simulation.focused_body_type = FocusedBodyType::SmallBody;
                                    state.simulation.focused_body_id = id;
                                }
                                if ui.button("Details").clicked() {
                                    state.sb_selected_id = Some(id);
                                }
                            });
                        }
                    }

                    if let Some(id) = state.sb_selected_id {
                        ui.separator();
                        ui.heading(format!("SB {}", id));

                        match sbdb::get_celestial_data(id) {
                            Ok(data) => {
                                if let Ok(pretty) = serde_json::to_string_pretty(&data) {
                                    ui.label(egui::RichText::new(pretty).monospace());
                                } else {
                                    ui.label(egui::RichText::new(format!("{}", data)).monospace());
                                }
                            }
                            Err(e) => {
                                ui.label(format!("Could not load data: {}", e));
                            }
                        }
                    }
                }
            });
        state.show_sb_manager = open;
    }

    if state.show_integrator_window {
        let mut open = state.show_integrator_window;
        egui::Window::new("Integrator")
            .open(&mut open)
            .resizable(false)
            .show(&ctx, |ui| {
                if let Some(sb_id) = state.integrator_sb_id {
                    if let Some(sb_name) = state
                        .simulation
                        .sb_celestial_objects
                        .get(&sb_id)
                        .map(|sb| sb.name.clone())
                    {
                        ui.horizontal(|ui| {
                            ui.label(format!("Body: {} (id={})", sb_name, sb_id));
                            let mut show_orbit = state.simulation.sb_orbit_filter.contains(&sb_id);
                            if ui.checkbox(&mut show_orbit, "Show preliminary orbit").changed() {
                                if show_orbit {
                                    state.simulation.sb_orbit_filter.insert(sb_id);
                                } else {
                                    state.simulation.sb_orbit_filter.remove(&sb_id);
                                }
                                rebuild_instances_and_gpu(state);
                            }
                        });
                        ui.separator();
                    } else {
                        ui.label(format!("Integrator target SB id {} is not in simulation", sb_id));
                        ui.separator();
                    }
                }
                ui.heading("Start time");
                ui.horizontal(|ui| {
                    ui.push_id("integrator_start_date", |ui| {
                        let mut date = NaiveDate::from_ymd_opt(
                            state.integrator_start.year,
                            state.integrator_start.month,
                            state.integrator_start.day,
                        )
                        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2000, 1, 1).unwrap());

                        let (year_min, year_max) = state.simulation.time_bounds_years();
                        if ui
                            .add(DatePickerButton::new(&mut date).start_end_years(year_min..=year_max))
                            .changed()
                        {
                            state.integrator_start.year = date.year();
                            state.integrator_start.month = date.month();
                            state.integrator_start.day = date.day();
                            sync_sim_time_to_integrator_start(state);
                        }
                    });
                    if ui
                        .add(
                            egui::DragValue::new(&mut state.integrator_start.hour)
                                .range(0..=23)
                                .prefix("H:"),
                        )
                        .changed()
                    {
                        sync_sim_time_to_integrator_start(state);
                    }
                    if ui
                        .add(
                            egui::DragValue::new(&mut state.integrator_start.minute)
                                .range(0..=59)
                                .prefix("M:"),
                        )
                        .changed()
                    {
                        sync_sim_time_to_integrator_start(state);
                    }
                    if ui
                        .add(
                            egui::DragValue::new(&mut state.integrator_start.second)
                                .range(0..=59)
                                .prefix("S:"),
                        )
                        .changed()
                    {
                        sync_sim_time_to_integrator_start(state);
                    }
                });

                ui.separator();
                ui.heading("End time");
                ui.horizontal(|ui| {
                    ui.push_id("integrator_end_date", |ui| {
                        let mut date = NaiveDate::from_ymd_opt(
                            state.integrator_end.year,
                            state.integrator_end.month,
                            state.integrator_end.day,
                        )
                        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2000, 1, 1).unwrap());

                        let (year_min, year_max) = state.simulation.time_bounds_years();
                        if ui
                            .add(DatePickerButton::new(&mut date).start_end_years(year_min..=year_max))
                            .changed()
                        {
                            state.integrator_end.year = date.year();
                            state.integrator_end.month = date.month();
                            state.integrator_end.day = date.day();
                        }
                    });
                    ui.add(
                        egui::DragValue::new(&mut state.integrator_end.hour)
                            .range(0..=23)
                            .prefix("H:"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut state.integrator_end.minute)
                            .range(0..=59)
                            .prefix("M:"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut state.integrator_end.second)
                            .range(0..=59)
                            .prefix("S:"),
                    );
                });

                ui.separator();
                if state.integrator_in_progress {
                    let frac = state.integrator_progress.clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .show_percentage()
                            .text("Integrating trajectory..."),
                    );
                }

                if ui.button("Run integration").clicked() {
                    if state.integrator_in_progress {
                        // ignore clicks while an integration is already running.
                    } else if state
                        .simulation
                        .sb_celestial_objects
                        .contains_key(&state.simulation.focused_body_id)
                    {
                        let sb_id = state.simulation.focused_body_id;
                        let dt_hours = state.integrator_dt_hours.max(0.001);
                        let dt = dt_hours * 3600.0;

                        let start_utc = state.integrator_start.utc_string();
                        let end_utc = state.integrator_end.utc_string();

                        if let (Ok(start_time), Ok(end_time)) = (
                            utils::utc_to_et(&start_utc),
                            utils::utc_to_et(&end_utc),
                        ) {
                            let (min_et, max_et) = state.simulation.time_bounds();
                            let start_time = start_time.clamp(min_et, max_et);
                            let end_time = end_time.clamp(min_et, max_et);
                            if end_time > start_time {
                                state.simulation.paused = true;
                                if state.simulation.integrator.is_some() {
                                    state.simulation.clear_integrator();
                                    state
                                        .trajectory_pipeline
                                        .update_vertices(&state.device, &state.queue, &Vec::<[f32; 3]>::new(), &Vec::<u32>::new(), None);
                                    state
                                        .closest_approach_pipeline
                                        .update_instances(&state.device, &state.queue, &Vec::<crate::modules::projection_3d::window_core::instance::Instance>::new(), true, false);
                                    state.closest_approach_pipeline
                                        .update_visibility(&state.queue, &Vec::<i32>::new(), &Vec::<crate::modules::projection_3d::window_core::instance::Instance>::new());
                                }
                                state.integrator_progress = 0.0;
                                if state.simulation.set_time(&start_utc).is_ok() {
                                    rebuild_instances_and_gpu(state);
                                }

                                let initial_state = state
                                    .simulation
                                    .sb_celestial_objects
                                    .get(&sb_id)
                                    .map(|sb| sb.glob_state)
                                    .unwrap_or(StateVector::default());

                                let coord_system = state.simulation.coord_system();
                                let naif_map = std::sync::Arc::clone(&state.simulation.naif_celestial_objects);
                                let proximity_reference = state.integrator_proximity_reference;
                                let proximity_multiplier = state.integrator_proximity_multiplier as f64;

                                let (tx_done, rx_done) = mpsc::channel::<Integrator>();
                                let (tx_prog, rx_prog) = mpsc::channel::<f32>();
                                state.integrator_in_progress = true;
                                state.integrator_result_rx = Some(rx_done);
                                state.integrator_progress_rx = Some(rx_prog);
                                let integrated_sb_id = sb_id;

                                std::thread::spawn(move || {
                                    let mut integrator = Integrator::new(
                                        integrated_sb_id,
                                        start_time,
                                        end_time,
                                        initial_state,
                                        dt,
                                        naif_map,
                                        proximity_reference,
                                        proximity_multiplier,
                                        coord_system,
                                    );

                                    integrator.run_with_progress(Some(&tx_prog));
                                    let _ = tx_done.send(integrator);
                                });
                            }
                        }
                    }
                }

                if !state.integrator_in_progress {
                    if let Some(integrator) = state.simulation.integrator.as_ref() {
                        let start_time = integrator.start_time;
                        let end_time = integrator.end_time;
                        let start_et = start_time;
                        ui.separator();
                        ui.horizontal(|ui| {
                            let propagation_active = !state.simulation.paused;
                            let propagation_label = if propagation_active {
                                "Pause propagation"
                            } else {
                                "Run propagation"
                            };

                            if ui.button(propagation_label).clicked() {
                                if propagation_active {
                                    state.simulation.paused = true;
                                } else {
                                    if state.simulation.current_time_et() < start_time
                                        || state.simulation.current_time_et() > end_time
                                    {
                                        if let Ok(start_utc) =
                                            utils::et_to_utc(start_et)
                                        {
                                            if state.simulation.set_time(&start_utc).is_ok() {
                                                rebuild_instances_and_gpu(state);
                                            }
                                        }
                                    }
                                    state.simulation.paused = false;
                                }
                            }

                            if ui.button("Set to start").clicked() {
                                if let Ok(start_utc) =
                                    utils::et_to_utc(start_et)
                                {
                                    if state.simulation.set_time(&start_utc).is_ok() {
                                        rebuild_instances_and_gpu(state);
                                    }
                                }
                            }
                        });
                    }

                    if let Some(integrator) = state.simulation.integrator.as_ref() {
                        let approaches = integrator.closest_approaches();
                        if !approaches.is_empty() {
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Closest approach results (approximately calculated by integrator):");
                                ui.checkbox(&mut state.show_closest_approach_markers, "Show markers");
                            });
                            for (idx, approach) in approaches.iter().enumerate() {
                                let body_name = state
                                    .simulation
                                    .naif_celestial_objects
                                    .get(&approach.body_id)
                                    .map(|body| body.name.clone())
                                    .unwrap_or_else(|| approach.body_id.to_string());
                                let body_radius = state
                                    .simulation
                                    .naif_celestial_objects
                                    .get(&approach.body_id)
                                    .and_then(|body| body.physical_params.as_ref().map(|p| p.radius()))
                                    .unwrap_or(0.0);
                                let distance_to_surface = approach.distance - body_radius;
                                let utc_time = utils::et_to_utc(approach.et)
                                    .unwrap_or_else(|_| "UTC conversion failed".to_string());
                                
                                let show_surface = state.closest_approach_show_surface_distance.contains(&idx);
                                let distance_display = if show_surface {
                                    format!("{} km to surface", format_with_commas(distance_to_surface, 3))
                                } else {
                                    format!("{} km to center", format_with_commas(approach.distance, 3))
                                };
                                
                                ui.horizontal(|ui| {
                                    ui.label(format!("#{}: {} {}", idx + 1, body_name, utc_time));
                                    let distance_button = ui.button(distance_display).on_hover_text("Click to toggle between surface and center distance");
                                    if distance_button.clicked() {
                                        if show_surface {
                                            state.closest_approach_show_surface_distance.remove(&idx);
                                        } else {
                                            state.closest_approach_show_surface_distance.insert(idx);
                                        }
                                    }
                                    ui.label(format!("rel vel: {} km/s", format_with_commas(approach.rel_velocity, 3)));
                                    if let Some(entry_et) = approach.high_proximity_entry_et {
                                        if ui.button("Set to entry").on_hover_text("Set time to the entry of high proximity phase").clicked() {
                                            if let Ok(entry_utc) = utils::et_to_utc(entry_et) {
                                                state.now_button_highlighted = false;
                                                if state.simulation.set_time(&entry_utc).is_ok() {
                                                    rebuild_instances_and_gpu(state);
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
            }
        );

        // when user closes the window via the close button, also clear integrator
        state.show_integrator_window = open;
        if !state.show_integrator_window {
            state.simulation.clear_integrator();
            state.trajectory_pipeline
				.update_vertices(&state.device, &state.queue, &Vec::<[f32;3]>::new(), &Vec::<u32>::new(), None);
            if let Some(sb_id) = state.integrator_sb_id { 
                if !state.simulation.sb_orbit_filter.contains(&sb_id) {
                    state.simulation.sb_orbit_filter.insert(sb_id);
                }
            }
            state.integrator_in_progress = false;
            state.integrator_result_rx = None;
            state.integrator_progress_rx = None;
            state.integrator_progress = 0.0;
            state.closest_approach_show_surface_distance.clear();
            state.simulation.paused = false;
        }

        // Handle completion of background integration, if any.
        if state.integrator_in_progress {
            if let Some(prog_rx) = &state.integrator_progress_rx {
                loop {
                    match prog_rx.try_recv() {
                        Ok(frac) => {
                            state.integrator_progress = frac.clamp(0.0, 1.0);
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            state.integrator_progress_rx = None;
                            break;
                        }
                    }
                }
            }

            if let Some(rx) = &state.integrator_result_rx {
                match rx.try_recv() {
                    Ok(integrator) => {

                        // MARK: plotting integrated trajectory
                        // could be optimized but should not be a problem
                        state.integrator_in_progress = false;
                        state.integrator_result_rx = None;
                        state.integrator_progress_rx = None;
                        state.integrator_progress = 1.0;
                        let sb_id = integrator.sb_id;
                        state.simulation.set_integrator(integrator);
                        state.cached_trajectory_flyby = None;
                        state.simulation.sb_orbit_filter.remove(&sb_id);
                        rebuild_instances_and_gpu(state);

                        if let Some((states, flags)) = state.simulation.trajectory_states_with_flags() {
                            let origin = state.render_origin;
                            let mut positions_f32: Vec<[f32; 3]> = Vec::with_capacity(states.len());
                            for state_at_epoch in states.iter() {
                                positions_f32.push([
                                    (state_at_epoch.state.position.x - origin.x) as f32,
                                    (state_at_epoch.state.position.y - origin.y) as f32,
                                    (state_at_epoch.state.position.z - origin.z) as f32,
                                ]);
                            }
                            state.trajectory_pipeline.update_vertices(
                                &state.device,
                                &state.queue,
                                &positions_f32,
                                flags,
                                None
                            );
                        }

                        if let Some(integrator) = state.simulation.integrator.as_ref() {
                            let closest_approaches = integrator.closest_approaches();
                            if !closest_approaches.is_empty() {
                                state.simulation.set_closest_approach_points(&closest_approaches, state.render_origin);
                                // Update the pipeline immediately so the closest approach markers are visible
                                state.closest_approach_pipeline.update_instances(&state.device, &state.queue, &state.simulation.closest_approach_instances, false, false);
                                state.closest_approach_pipeline.update_visibility(&state.queue, &state.simulation.closest_approach_visible_ids(), &state.simulation.closest_approach_instances);
                            }
                        }

                    }
                    Err(TryRecvError::Empty) => {
                        // Still running; nothing to do.
                    }
                    Err(TryRecvError::Disconnected) => {
                        state.integrator_in_progress = false;
                        state.integrator_result_rx = None;
                    }
                }
            }
            
        }
        
        
    }

    match state.simulation.focused_body_type {
        crate::modules::projection_3d::simulation::FocusedBodyType::Naif => {
            if let Some(body) = state
                .simulation
                .naif_celestial_objects
                .get(&state.simulation.focused_body_id)
            {
                let gp = body.rel_state;
                let distance = gp.position.magnitude();
                let speed = gp.velocity.magnitude();

                egui::Window::new("Celestial Info").show(&ctx, |ui| {
                    ui.label(format!("{} (id: {})", body.name, body.id));
                    ui.label(format!("|r| = {} km", format_with_commas(distance, 3)));
                    ui.label(format!("|v| = {} km/s", format_with_commas(speed, 6)));
                    if let Some(kep) = &body.kep_elts {
                        ui.separator();
                        ui.label("Keplerian Elements:");
                        ui.label(format!(
                            "a = {} km",
                            format_with_commas(kep.a.round(), 0)
                        ));
                        ui.label(format!("e = {}", format_with_commas(kep.ecc, 6)));
                        ui.label(format!(
                            "i = {}°",
                            format_with_commas(kep.inc.to_degrees(), 3)
                        ));
                        ui.label(format!(
                            "Ω = {}°",
                            format_with_commas(kep.lnode.to_degrees(), 3)
                        ));
                        ui.label(format!(
                            "ω = {}°",
                            format_with_commas(kep.argp.to_degrees(), 3)
                        ));
                    }
                    // Show SB distance to this body if in integration mode
                    if let Some(integrator) = &state.simulation.integrator {
                        ui.separator();
                        let sb_state = integrator.state_at(state.simulation.current_time_et());
                        let sb_pos = sb_state.position;
                        let sb_vel = sb_state.velocity;
                        let body_pos = body.glob_state.position;
                        let body_vel = body.rel_state.velocity;
                        let dx = sb_pos[0] - body_pos[0];
                        let dy = sb_pos[1] - body_pos[1];
                        let dz = sb_pos[2] - body_pos[2];
                        let dvx = sb_vel[0] - body_vel[0];
                        let dvy = sb_vel[1] - body_vel[1];
                        let dvz = sb_vel[2] - body_vel[2];
                        let distance_to_center = (dx * dx + dy * dy + dz * dz).sqrt();
                        let rel_vel = (dvx * dvx + dvy * dvy + dvz * dvz).sqrt();
                        let body_radius = body.physical_params.as_ref().map(|p| p.radius()).unwrap_or(0.0);
                        let distance_to_surface = distance_to_center - body_radius;
                        ui.label("Distance from integrated SB:");
                        ui.label(format!(
                            "  Center: {} km",
                            format_with_commas(distance_to_center, 3)
                        ));
                        ui.label(format!(
                            "  Surface: {} km",
                            format_with_commas(distance_to_surface, 3)
                        ));
                        ui.label(format!(
                            "  Rel vel: {} km/s",
                            format_with_commas(rel_vel, 3)
                        ));
                    }
                });
            } else {
                egui::Window::new("Celestial Info").show(&ctx, |ui| {
                    ui.label("No focused body");
                });
            }
        }
        crate::modules::projection_3d::simulation::FocusedBodyType::SmallBody => {
            if let Some(sb) = state
                .simulation
                .sb_celestial_objects
                .get(&state.simulation.focused_body_id)
            {
                let gs = sb.glob_state;
                let distance = gs.position.magnitude();
                let speed = gs.velocity.magnitude();
                let sb_name = sb.name.clone();
                let sb_id = sb.id;
                let kep_elts = sb.kep_elts;

                egui::Window::new("Celestial Info").show(&ctx, |ui| {
                    ui.label(format!(
                        "SB: {} (id={})",
                        sb_name, sb_id
                    ));
                    ui.label(format!(
                        "|r| = {} km",
                        format_with_commas(distance, 3)
                    ));
                    ui.label(format!(
                        "|v| = {} km/s",
                        format_with_commas(speed, 6)
                    ));
                    ui.horizontal(|ui| {
                        ui.label("Distance target:");
                        let target_label = state
                            .simulation
                            .naif_celestial_objects
                            .get(&state.sbdb_naif_distance_target_id)
                            .map(|target| format!("{} ({})", target.name, target.id))
                            .unwrap_or_else(|| format!("Unknown ({})", state.sbdb_naif_distance_target_id));
                        ui.label(target_label);
                        if ui.button("Choose...").clicked() {
                            state.show_sbdb_naif_search = true;
                        }
                    });

                    if state.show_sbdb_naif_search {
                        let mut selected_target: Option<i32> = None;
                        egui::Window::new("Choose NAIF reference")
                            .resizable(true)
                            .default_size([400.0, 400.0])
                            .open(&mut state.show_sbdb_naif_search)
                            .show(&ctx, |ui| {
                                ui.label("Search by name or ID:");
                                ui.text_edit_singleline(&mut state.sbdb_naif_search);
                                ui.separator();

                                egui::ScrollArea::vertical()
                                    .auto_shrink([false; 2])
                                    .show(ui, |ui| {
                                        let search = state.sbdb_naif_search.to_lowercase();
                                        for naif_id in &state.simulation.naif_sorted_ids {
                                            if let Some(naif) = state.simulation.naif_celestial_objects.get(naif_id) {
                                                let row_label = format!("{} ({})", naif.name, naif.id);
                                                let row_text = row_label.to_lowercase();
                                                if !search.is_empty() && !row_text.contains(&search) {
                                                    continue;
                                                }

                                                if ui
                                                    .selectable_label(
                                                        state.sbdb_naif_distance_target_id == naif.id,
                                                        row_label,
                                                    )
                                                    .clicked()
                                                {
                                                    selected_target = Some(naif.id);
                                                }
                                            }
                                        }
                                    });
                            });
                        if let Some(target_id) = selected_target {
                            state.sbdb_naif_distance_target_id = target_id;
                            state.show_sbdb_naif_search = false;
                        }
                    }

                    if let Some(target) = state
                        .simulation
                        .naif_celestial_objects
                        .get(&state.sbdb_naif_distance_target_id)
                    {
                        let target_state = target.glob_state;
                        let rel_state = gs - target_state;
                        let distance_to_center = rel_state.position.magnitude();
                        let rel_vel = rel_state.velocity.magnitude();
                        let target_radius = target.physical_params.as_ref().map(|p| p.radius()).unwrap_or(0.0);
                        let distance_to_surface = distance_to_center - target_radius;
                        ui.label(format!(
                            "Distance to {}: {} km (surface)",
                            target.name,
                            format_with_commas(distance_to_surface, 3)
                        ));
                        ui.label(format!(
                            "Rel vel to {}: {} km/s",
                            target.name,
                            format_with_commas(rel_vel, 3)
                        ));
                    }

                    ui.separator();
                    ui.label("Keplerian Elements:");
                    ui.label(format!(
                        "a = {} km",
                        format_with_commas(kep_elts.a.round(), 0)
                    ));
                    ui.label(format!("e = {}", format_with_commas(kep_elts.ecc, 6)));
                    ui.label(format!(
                        "i = {}°",
                        format_with_commas(kep_elts.inc.to_degrees(), 3)
                    ));
                    ui.label(format!(
                        "Ω = {}°",
                        format_with_commas(kep_elts.lnode.to_degrees(), 3)
                    ));
                    ui.label(format!(
                        "ω = {}°",
                        format_with_commas(kep_elts.argp.to_degrees(), 3)
                    ));

						ui.separator();
						if ui.button("Integrate").clicked() {
							state.integrator_sb_id = Some(sb_id);
                            // Initialize integrator start/end from current simulation UTC (always "YYYY-MM-DD HH:MM:SS.mmm").
                            let cur_utc = state.simulation.current_time_utc().to_string();
                            let truncated = &cur_utc[..std::cmp::min(cur_utc.len(), 19)];
                            let parsed = NaiveDateTime::parse_from_str(truncated, "%Y-%m-%d %H:%M:%S");
                            if let Ok(dt) = parsed {
                                state.integrator_start = YMDHMS::from_datetime(dt);
                                state.integrator_end = YMDHMS::from_datetime(dt + Duration::days(150));
                            }

                            state.simulation.paused = true;
                            state.now_button_highlighted = false;
                            state.show_integrator_window = true;
                        }
                });
            } else {
                egui::Window::new("Celestial Info").show(&ctx, |ui| {
                    ui.label("No focused SB");
                });
            }
        }
    }

    if state.settings_dirty {
        save_settings(state);
        state.settings_dirty = false;
    }
}
