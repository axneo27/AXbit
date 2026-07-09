struct Camera {
    view_pos: vec4<f32>,
    view: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: Camera;

// Per-point trajectory data used for tangent computation
struct TrajPoint {
    position: vec3<f32>,
    flag: u32,
};

@group(1) @binding(0)
var<storage, read> traj_points: array<TrajPoint>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>, // y = direction (+1 or -1)
};

struct VSOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(v: VertexInput, @builtin(vertex_index) vertex_index: u32) -> VSOut {
    const astro_to_graphics: mat3x3<f32> = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 1.0, 0.0),
    );

    // Each logical point is duplicated with tex_coords.y = +/-1
    let logical_idx: u32 = vertex_index / 2u;
    let dir_sign: f32 = v.tex_coords.y;

    let point_count: u32 = arrayLength(&traj_points);
    // Guard against empty buffers (should be skipped by CPU-side checks)
    let last_idx: u32 = max(point_count, 1u) - 1u;

    let prev_idx: u32 = select(logical_idx, logical_idx - 1u, logical_idx > 0u);
    let next_idx: u32 = select(logical_idx, logical_idx + 1u, logical_idx < last_idx);

    let p_prev: vec3<f32> = traj_points[prev_idx].position;
    let p_curr: vec3<f32> = traj_points[logical_idx].position;
    let p_next: vec3<f32> = traj_points[next_idx].position;

    // Project three points to NDC to get screen-space direction
    let world_prev: vec4<f32> = vec4<f32>(astro_to_graphics * p_prev, 1.0);
    let world_curr: vec4<f32> = vec4<f32>(astro_to_graphics * p_curr, 1.0);
    let world_next: vec4<f32> = vec4<f32>(astro_to_graphics * p_next, 1.0);

    let clip_prev: vec4<f32> = camera.view_proj * world_prev;
    let clip_curr: vec4<f32> = camera.view_proj * world_curr;
    let clip_next: vec4<f32> = camera.view_proj * world_next;

    var ndc_prev: vec2<f32> = clip_prev.xy / clip_prev.w;
    var ndc_curr: vec2<f32> = clip_curr.xy / clip_curr.w;
    var ndc_next: vec2<f32> = clip_next.xy / clip_next.w;

    let aspect: f32 = camera.inv_proj[0][0] / camera.inv_proj[1][1];
    ndc_prev.x *= aspect;
    ndc_curr.x *= aspect;
    ndc_next.x *= aspect;

    let dir: vec2<f32> = normalize(ndc_next - ndc_prev);
    var normal: vec2<f32> = vec2<f32>(-dir.y, dir.x);

    // thickness in NDC (larger than orbit)
    let thickness: f32 = 0.0072;
    normal *= thickness / 2.0;
    normal.x /= aspect;

    // offset clip position by NDC offset scaled back to clip space
    let offset_clip: vec4<f32> = vec4<f32>(normal * dir_sign * clip_curr.w, 0.0, 0.0);

    var out: VSOut;
    out.clip_position = clip_curr + offset_clip;

    let C: f32 = 10.0;
    out.clip_position.z = log2(max(1e-6, 1.0 + out.clip_position.w)) * C - 1.0;

    // red when close, green otherwise
    let is_close: u32 = traj_points[logical_idx].flag;
    if is_close == 1u {
        out.color = vec3<f32>(1.0, 0.0, 0.0);
    } else {
        out.color = vec3<f32>(0.0, 1.0, 0.0);
    }
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
