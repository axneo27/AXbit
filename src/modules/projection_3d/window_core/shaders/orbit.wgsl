struct Camera {
    view_pos: vec4<f32>,
    view: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
}
@group(0) @binding(0)
var<uniform> camera: Camera;

struct ModelVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>, // x = t [0..1], y = direction (+1 or -1)
}
struct InstanceInput {
    model_matrix: mat4x4<f32>,
    color: vec4<f32>,
    id: vec4<u32>,
};

@group(1) @binding(0) var<storage, read> all_instances: array<InstanceInput>;
@group(1) @binding(1) var<storage, read> visible_indices: array<u32>;

struct VSOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) color: vec3<f32>,
}

@vertex
fn vs_main(model: ModelVertexInput, @builtin(instance_index) instance_index: u32) -> VSOut {
    let real_idx = visible_indices[instance_index];
    let instance = all_instances[real_idx];
    let model_matrix = instance.model_matrix;

    const astro_to_graphics: mat3x3<f32> = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 1.0, 0.0),
    );

    // screen-space thick line
    let t: f32 = model.tex_coords.x;
    let dir_sign: f32 = model.tex_coords.y; // +1 or -1

    let ang: f32 = 6.283185307179586 * t; 
    let s: f32 = sin(ang);
    let c: f32 = cos(ang);
    let local_pos: vec3<f32> = vec3<f32>(c, s, 0.0);
    let world_pos: vec4<f32> = model_matrix * vec4<f32>(local_pos, 1.0);
    let transformed_world_pos: vec4<f32> = vec4<f32>(astro_to_graphics * world_pos.xyz, world_pos.w);
    let clip_curr: vec4<f32> = camera.view_proj * transformed_world_pos;
    var ndc_curr: vec2<f32> = clip_curr.xy / clip_curr.w;

    let d_ang: f32 = 0.002;
    let s_n: f32 = sin(ang + d_ang);
    let c_n: f32 = cos(ang + d_ang);
    let local_next: vec3<f32> = vec3<f32>(c_n, s_n, 0.0);
    let world_next: vec4<f32> = model_matrix * vec4<f32>(local_next, 1.0);
    let transformed_world_next: vec4<f32> = vec4<f32>(astro_to_graphics * world_next.xyz, world_next.w);
    let clip_next: vec4<f32> = camera.view_proj * transformed_world_next;
    var ndc_next: vec2<f32> = clip_next.xy / clip_next.w;

    let aspect: f32 = camera.inv_proj[0][0] / camera.inv_proj[1][1];
    ndc_curr.x *= aspect;
    ndc_next.x *= aspect;

    let dir: vec2<f32> = normalize(ndc_next - ndc_curr);
    var normal: vec2<f32> = vec2<f32>(-dir.y, dir.x);

    // thickness in NDC 
    let thickness: f32 = 0.0032;
    normal *= thickness / 2.0;
    normal.x /= aspect;

    // offset clip position by NDC offset scaled back to clip space
    let offset_clip: vec4<f32> = vec4<f32>(normal * dir_sign * clip_curr.w, 0.0, 0.0);

    var out: VSOut;
    out.clip_position = clip_curr + offset_clip;
    let C: f32 = 10.0;
    out.clip_position.z = log2(max(1e-6, 1.0 + out.clip_position.w)) * C - 1.0;

    out.world_position = transformed_world_pos.xyz;
    out.color = instance.color.xyz;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}