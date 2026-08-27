use super::super::window::{AppState, SbManagerTab};
use crate::modules::projection_3d::simulation::FocusedBodyType;
use crate::modules::sbdb;
use crate::modules::{
    spice_ker,
    utils::{self, DistanceUnit, YMDHMS},
};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use egui_extras::DatePickerButton;
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;

use super::common::{format_with_commas, rebuild_instances_and_gpu};

const SECONDS_PER_DAY: f64 = 86400.0;
const CLOSE_APPROACH_DAYS_BEFORE: i64 = 75;
const CLOSE_APPROACH_DAYS_AFTER: i64 = 7;
const DRAGVALUE_BASE_SPEED_AU: f64 = 0.00001;

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
                        Err(e) => {
                            state.sb_status = Some(format!("Failed to add SB to simulation: {}", e))
                        }
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
        let result = sbdb::download_and_store_small_body(&search).map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
}

fn prepare_close_approach_integration(
    state: &mut AppState,
    small_body_id: i32,
    close_approach: &sbdb::CloseApproachData,
) -> Result<(), String> {
    let tca = utils::jd_to_et(close_approach.tca_jd).map_err(|e| e.to_string())?;

    // Give the integrator time to propagate toward the encounter, then keep a
    // short period after TCA so the complete flyby is visible.
    let start_et = tca - CLOSE_APPROACH_DAYS_BEFORE as f64 * SECONDS_PER_DAY;
    let start_utc = utils::et_to_utc(start_et).map_err(|e| e.to_string())?;

    let truncated = &start_utc[..std::cmp::min(start_utc.len(), 19)];
    let start =
        NaiveDateTime::parse_from_str(truncated, "%Y-%m-%d %H:%M:%S").map_err(|e| e.to_string())?;
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

pub(super) fn show(state: &mut AppState, ctx: &egui::Context) {
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
}
