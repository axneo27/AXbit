struct Camera {
    view_pos: vec4<f32>,
    view: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
}
@group(0) @binding(0)
var<uniform> camera: Camera;

//@group(1)
// @binding(0)
// var env_map: texture_cube<f32>;
// @binding(1)
// var env_sampler: sampler;

struct VertexOutput {
    @builtin(position) frag_position: vec4<f32>,
    @location(0) clip_position: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) id: u32,
) -> VertexOutput {
    let uv = vec2<f32>(vec2<u32>(
        id & 1u,
        (id >> 1u) & 1u,
    ));
    var out: VertexOutput;
    // out.clip_position = vec4(uv * vec2(4.0, -4.0) + vec2(-1.0, 1.0), 0.0, 1.0);
    // For reverse-Z: place sky at far depth (z = 0.0)
    out.clip_position = vec4(uv * 4.0 - 1.0, 0.0, 1.0);
    out.frag_position = vec4(uv * 4.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Reconstruct a stable view-space direction that is insensitive to znear/zfar.
    // Use a fixed clip/NDC z of 1.0 for direction; depth test comes from frag_position.z = 0.0.
    let view_pos_homogeneous = camera.inv_proj * vec4(in.clip_position.xy, 1.0, 1.0);
    let view_ray_direction = view_pos_homogeneous.xyz / view_pos_homogeneous.w;
    var ray_direction = normalize((camera.inv_view * vec4(view_ray_direction, 0.0)).xyz);

    // let sample = textureSample(env_map, env_sampler, ray_direction);
    // return sample;

    // Return dark background
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
