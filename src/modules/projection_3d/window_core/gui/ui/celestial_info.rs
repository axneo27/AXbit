use super::super::window::AppState;
use super::common::format_with_commas;
use crate::modules::utils::YMDHMS;
use cgmath::InnerSpace;
use chrono::{Duration, NaiveDateTime};

pub(super) fn show(state: &mut AppState, ctx: &egui::Context) {
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
                        ui.label(format!("a = {} km", format_with_commas(kep.a.round(), 0)));
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
                        let body_radius = body
                            .physical_params
                            .as_ref()
                            .map(|p| p.radius())
                            .unwrap_or(0.0);
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
                    ui.label(format!("SB: {} (id={})", sb_name, sb_id));
                    ui.label(format!("|r| = {} km", format_with_commas(distance, 3)));
                    ui.label(format!("|v| = {} km/s", format_with_commas(speed, 6)));
                    ui.horizontal(|ui| {
                        ui.label("Distance target:");
                        let target_label = state
                            .simulation
                            .naif_celestial_objects
                            .get(&state.sbdb_naif_distance_target_id)
                            .map(|target| format!("{} ({})", target.name, target.id))
                            .unwrap_or_else(|| {
                                format!("Unknown ({})", state.sbdb_naif_distance_target_id)
                            });
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

                                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(
                                    ui,
                                    |ui| {
                                        let search = state.sbdb_naif_search.to_lowercase();
                                        for naif_id in &state.simulation.naif_sorted_ids {
                                            if let Some(naif) =
                                                state.simulation.naif_celestial_objects.get(naif_id)
                                            {
                                                let row_label =
                                                    format!("{} ({})", naif.name, naif.id);
                                                let row_text = row_label.to_lowercase();
                                                if !search.is_empty() && !row_text.contains(&search)
                                                {
                                                    continue;
                                                }

                                                if ui
                                                    .selectable_label(
                                                        state.sbdb_naif_distance_target_id
                                                            == naif.id,
                                                        row_label,
                                                    )
                                                    .clicked()
                                                {
                                                    selected_target = Some(naif.id);
                                                }
                                            }
                                        }
                                    },
                                );
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
                        let target_radius = target
                            .physical_params
                            .as_ref()
                            .map(|p| p.radius())
                            .unwrap_or(0.0);
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
}
