struct Camera {
    view_pos: vec4<f32>,
    view: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct Uniforms {
    viewport_size: vec2<f32>,
};
@group(0) @binding(1) var<uniform> uniforms: Uniforms;

// ====================== GPU-driven instance data ======================
struct Instance {
    model_matrix: mat4x4<f32>,
    color: vec4<f32>,
    id: vec4<u32>,      
};

@group(1) @binding(0) var<storage, read> all_instances: array<Instance>;
@group(1) @binding(1) var<storage, read> visible_indices: array<u32>;
// ===========================================================================

struct ModelVertexInput {
    @location(0) position: vec3<f32>,   // quad offset in [-0.5..0.5], z=0
};

struct VSOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) unit_pos: vec2<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_main(
    model: ModelVertexInput,
    @builtin(instance_index) instance_index: u32,   // ← 0 .. num_visible-1
) -> VSOut {

    let real_idx = visible_indices[instance_index];
    let inst = all_instances[real_idx];

    let model_matrix = inst.model_matrix; 

    const astro_to_graphics: mat3x3<f32> = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 1.0, 0.0),
    );

    let center_world = model_matrix * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let transformed_center_world = vec4<f32>(astro_to_graphics * center_world.xyz, center_world.w);
    let center_clip = camera.view_proj * transformed_center_world;
    let center_ndc = center_clip.xyz / center_clip.w;

    let pixel_radius = 30.0;
    let offset_ndc = model.position.xy * (2.0 * pixel_radius / uniforms.viewport_size);

    let final_ndc_xy = center_ndc.xy + offset_ndc;

    // tiny, deterministic per-instance depth jitter (NDC units) to break ties
    // between markers that are exactly coplanar and avoid z-fighting/flicker.
    // For planetary markers (priority flag in inst.id[1]) add a larger bias so
    // they always render above non‑planet markers while keeping per‑id jitter.
    let idf = f32(inst.id[0]);
    var depth_jitter = (fract(idf * 0.6180339887498948) - 0.5) * 1e-4;
    if (inst.id[1] > 0u) {
        // reversed-Z compare (GreaterEqual) — increase NDC z to bring forward
        depth_jitter = depth_jitter + 1e-3; // priority bias
    }
    let final_ndc_z  = center_ndc.z + depth_jitter;

    let final_clip = vec4<f32>(
        final_ndc_xy * center_clip.w,
        final_ndc_z * center_clip.w,
        center_clip.w
    );

    var out: VSOut;
    out.clip_position = final_clip;
    out.unit_pos = model.position.xy;
    out.color = inst.color.xyz;          // now from storage buffer
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let dist = length(in.unit_pos);
    if (dist > 0.5) { discard; }

    let edge = 0.05;
    let alpha = smoothstep(0.5, 0.5 - edge, dist);

    return vec4<f32>(in.color, alpha);
}