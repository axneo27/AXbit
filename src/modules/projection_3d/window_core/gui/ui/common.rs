use super::super::window::AppState;
use std::format;

pub(super) fn format_with_commas(val: f64, decimals: usize) -> String {
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

pub(super) fn rebuild_instances_and_gpu(state: &mut AppState) {
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
