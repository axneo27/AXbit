struct Camera {
    view_pos: vec4<f32>,
    view: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
}
@group(1) @binding(0)
var<uniform> camera: Camera;

struct Light {
    position: vec3<f32>,
    color: vec3<f32>,
}
@group(2) @binding(0)
var<uniform> light: Light;

struct ModelVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) tangent: vec3<f32>,
    @location(4) bitangent: vec3<f32>,
}
struct InstanceInput {
    @location(5) model_matrix_0: vec4<f32>,
    @location(6) model_matrix_1: vec4<f32>,
    @location(7) model_matrix_2: vec4<f32>,
    @location(8) model_matrix_3: vec4<f32>,
    @location(9) normal_matrix_0: vec3<f32>,
    @location(10) normal_matrix_1: vec3<f32>,
    @location(11) normal_matrix_2: vec3<f32>,
}

struct ModelVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) world_normal: vec3<f32>,
    @location(3) world_tangent: vec3<f32>,
}

@vertex
fn vs_main(
    model: ModelVertexInput,
    instance: InstanceInput,
) -> ModelVertexOutput {
    let model_matrix = mat4x4<f32>(
        instance.model_matrix_0,
        instance.model_matrix_1,
        instance.model_matrix_2,
        instance.model_matrix_3,
    );
    let normal_matrix = mat3x3<f32>(
        instance.normal_matrix_0,
        instance.normal_matrix_1,
        instance.normal_matrix_2,
    );

    const astro_to_graphics: mat3x3<f32> = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 1.0, 0.0),
    );

    var world_position = model_matrix * vec4<f32>(model.position, 1.0);
    world_position = vec4<f32>(astro_to_graphics * world_position.xyz, world_position.w);

    var out: ModelVertexOutput;
    out.clip_position = camera.view_proj * world_position;
    out.tex_coords = model.tex_coords;
    out.world_normal = normalize(astro_to_graphics * (normal_matrix * model.normal));
    out.world_tangent = normalize(astro_to_graphics * (normal_matrix * model.tangent));
    let C: f32 = 10.0;
    world_position.z = log2(max(1e-6, 1.0 + world_position.w)) * C - 1.0;
    world_position.z *= world_position.w;

    out.world_position = world_position.xyz;

    return out;
}

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0) @binding(1)
var s_diffuse: sampler;
@group(0) @binding(2)
var t_normal: texture_2d<f32>;
@group(0) @binding(3)
var s_normal: sampler;

@fragment
fn fs_main(in: ModelVertexOutput) -> @location(0) vec4<f32> {
    let object_color = textureSample(t_diffuse, s_diffuse, in.tex_coords);
    let object_normal = textureSample(t_normal, s_normal, in.tex_coords);
    
    // TBN matrix
    let tangent = normalize(in.world_tangent - dot(in.world_tangent, in.world_normal) * in.world_normal);
    let bitangent = cross(tangent, in.world_normal);
    let TBN = mat3x3(tangent, bitangent, in.world_normal);
    
    // Normal in world space
    let world_normal = normalize(TBN * (object_normal.xyz * 2.0 - 1.0));
    
    // Lighting vectors
    let light_vec = light.position - in.world_position;
    let light_dir = normalize(light_vec);
    let view_dir = normalize(camera.view_pos.xyz - in.world_position);
    let half_dir = normalize(view_dir + light_dir);
    
    // Basic Phong
    let ambient = light.color * 0.1;
    let diffuse = light.color * max(dot(world_normal, light_dir), 0.0);
    let specular = light.color * pow(max(dot(world_normal, half_dir), 0.0), 32.0);
    
    let result = (ambient + diffuse + specular) * object_color.xyz;
    return vec4<f32>(result, object_color.a);
}
