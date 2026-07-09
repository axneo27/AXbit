struct Camera {
    view_pos: vec4<f32>,
    view: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
}
@group(0) @binding(0)
var<uniform> camera: Camera;

struct Light {
    position: vec3<f32>,
    color: vec3<f32>,
}
@group(1) @binding(0)
var<uniform> light: Light;

struct ModelVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) tangent: vec3<f32>,
    @location(4) bitangent: vec3<f32>,
}

// storage-backed instance layout (matches Rust `BodyInstanceStorage`)
struct BodyInstance {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    color: vec4<f32>,
    id: vec4<u32>,
}

@group(2) @binding(0)
var<storage, read> all_instances: array<BodyInstance>;

@group(2) @binding(1)
var<storage, read> visible_indices: array<u32>;

struct ModelVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) world_normal: vec3<f32>,
    @location(3) world_tangent: vec3<f32>,
}

@vertex
fn vs_main(
    model: ModelVertexInput,
    @builtin(instance_index) instance_idx: u32,
) -> ModelVertexOutput {
    let inst_index = visible_indices[instance_idx];
    let instance = all_instances[inst_index];
    let model_matrix = instance.model;
    let normal_matrix = mat3x3<f32>(
        instance.normal[0].xyz,
        instance.normal[1].xyz,
        instance.normal[2].xyz,
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
    let C: f32 = 10.0;
    out.clip_position.z = log2(max(1e-6, 1.0 + out.clip_position.w)) * C - 1.0;
    out.color = instance.color.xyz;
    out.world_normal = normalize(astro_to_graphics * (normal_matrix * model.normal));
    out.world_tangent = normalize(astro_to_graphics * (normal_matrix * model.tangent));

    out.world_position = world_position.xyz;

    return out;
}

@fragment
fn fs_main(in: ModelVertexOutput) -> @location(0) vec4<f32> {
    let object_color = vec4<f32>(in.color, 1.0);
    let world_normal = in.world_normal;
    
    // Transform light position to graphics coordinates
    const astro_to_graphics: mat3x3<f32> = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let light_pos_graphics = astro_to_graphics * light.position;
    
    // Transform camera position to graphics coordinates
    let camera_pos_graphics = astro_to_graphics * camera.view_pos.xyz;
    
    // Lighting vectors
    let light_vec = light_pos_graphics - in.world_position;
    let light_dir = normalize(light_vec);
    let view_dir = normalize(camera_pos_graphics - in.world_position);
    let half_dir = normalize(view_dir + light_dir);
    
    // Basic Phong
    let ambient = light.color * 0.1;
    let diffuse = light.color * max(dot(world_normal, light_dir), 0.0);
    let specular = light.color * pow(max(dot(world_normal, half_dir), 0.0), 32.0);
    
    let result = (ambient + diffuse + specular) * object_color.xyz;
    return vec4<f32>(result, object_color.a);
}