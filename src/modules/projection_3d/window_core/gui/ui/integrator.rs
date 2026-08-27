use super::super::window::AppState;
use crate::modules::projection_3d::traj::Integrator;
use crate::modules::{projection_3d::state::StateVector, utils};
use chrono::{Datelike, NaiveDate};
use egui_extras::DatePickerButton;
use std::sync::mpsc;
use std::sync::mpsc::TryRecvError;

use super::common::{format_with_commas, rebuild_instances_and_gpu};

fn sync_sim_time_to_integrator_start(state: &mut AppState) {
    let utc = state.integrator_start.utc_string();
    if state.simulation.set_time(&utc).is_ok() {
        rebuild_instances_and_gpu(state);
    }
}

pub(super) fn show(state: &mut AppState, ctx: &egui::Context) {
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

                                    let distance_button = ui.button(distance_display)
                                        .on_hover_text("Click to toggle between surface and center distance");

                                    if distance_button.clicked() {
                                        if show_surface {
                                            state.closest_approach_show_surface_distance.remove(&idx);
                                        } else {
                                            state.closest_approach_show_surface_distance.insert(idx);
                                        }
                                    }
                                    ui.label(format!("rel vel: {} km/s", format_with_commas(approach.rel_velocity, 3)));
                                    if let Some(entry_et) = approach.high_proximity_entry_et {
                                        
                                        if ui.button("Set to entry")
                                            .on_hover_text("Set time to the entry of high proximity phase")
                                            .clicked() {
                                            if let Ok(entry_utc) = utils::et_to_utc(entry_et) {
                                                state.now_button_highlighted = false;
                                                if state.simulation.set_time(&entry_utc).is_ok() {
                                                    rebuild_instances_and_gpu(state);
                                                }
                                            }
                                        }

                                        // if ui.button("Compare with JPL")
                                        //     .on_hover_text("Compare this close approach data with SBDB JPL values")
                                        //     .clicked() {


                                        // }

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
            state.trajectory_pipeline.update_vertices(
                &state.device,
                &state.queue,
                &Vec::<[f32; 3]>::new(),
                &Vec::<u32>::new(),
                None,
            );
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

                        if let Some((states, flags)) =
                            state.simulation.trajectory_states_with_flags()
                        {
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
                                None,
                            );
                        }

                        if let Some(integrator) = state.simulation.integrator.as_ref() {
                            let closest_approaches = integrator.closest_approaches();
                            if !closest_approaches.is_empty() {
                                state.simulation.set_closest_approach_points(
                                    &closest_approaches,
                                    state.render_origin,
                                );
                                // Update the pipeline immediately so the closest approach markers are visible
                                state.closest_approach_pipeline.update_instances(
                                    &state.device,
                                    &state.queue,
                                    &state.simulation.closest_approach_instances,
                                    false,
                                    false,
                                );
                                state.closest_approach_pipeline.update_visibility(
                                    &state.queue,
                                    &state.simulation.closest_approach_visible_ids(),
                                    &state.simulation.closest_approach_instances,
                                );
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
}
