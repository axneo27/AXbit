
use wgpu::util::DeviceExt;

use crate::modules::projection_3d::window_core::{model::{Material, Mesh, Model, ModelVertex}, texture};

pub struct MeshData {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u32>,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrawParams {
    vertex_count_per_mesh: u32,
    _padding: [u32; 3],
}

impl DrawParams {
    pub fn new(vc: u32) -> Self {
        DrawParams { vertex_count_per_mesh: vc, _padding: [0; 3] }
    }
}

pub fn create_solid_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
    label: &str,
    is_normal_map: bool,
) -> texture::Texture {
    let size = wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 };
    let format = if is_normal_map { wgpu::TextureFormat::Rgba8Unorm } else { wgpu::TextureFormat::Rgba8UnormSrgb };
    let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            aspect: wgpu::TextureAspect::All,
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
        size,
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    texture::Texture { texture, view, sampler, size }
}

pub fn create_quad_model(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    color_rgba: [u8; 4],
) -> Model {
    let mesh_data = create_quad_mesh_data();
    let mesh = create_mesh(device, &mesh_data, "quad", 0);

    let diffuse = create_solid_texture(device, queue, color_rgba, "quad_diffuse", false);
    let normal_flat = create_solid_texture(device, queue, [128, 128, 255, 255], "quad_normal", true);

    let material = Material::new(device, "quad_material", diffuse, normal_flat, layout);

    Model { meshes: vec![mesh], materials: vec![material] }
}

pub fn create_mesh(device: &wgpu::Device, data: &MeshData, name: &str, material_index: usize) -> Mesh {
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{} Vertex Buffer", name)),
        contents: bytemuck::cast_slice(&data.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{} Index Buffer", name)),
        contents: bytemuck::cast_slice(&data.indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    Mesh {
        name: name.to_string(),
        vertex_buffer,
        index_buffer,
        num_elements: data.indices.len() as u32,
        material: material_index,
    }
}

pub fn create_quad_mesh_data() -> MeshData {
    let vertices = vec![
        ModelVertex {
            position: [-0.5, -0.5, 0.0],
            tex_coords: [0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        },
        ModelVertex {
            position: [0.5, -0.5, 0.0],
            tex_coords: [1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        },
        ModelVertex {
            position: [0.5, 0.5, 0.0],
            tex_coords: [1.0, 1.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        },
        ModelVertex {
            position: [-0.5, 0.5, 0.0],
            tex_coords: [0.0, 1.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        },
    ];

    let indices = vec![
        0, 1, 2,
        2, 3, 0,
    ];

    MeshData { vertices, indices }
}

pub fn create_render_pipeline_default(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    depth_format: Option<wgpu::TextureFormat>,
    vertex_layouts: &[wgpu::VertexBufferLayout],
    topology: wgpu::PrimitiveTopology,
    shader: wgpu::ShaderModuleDescriptor,
    depth_write_enabled: bool,
    depth_compare: wgpu::CompareFunction,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(shader);

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Render Pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: vertex_layouts,
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState { //////
                    alpha: wgpu::BlendComponent::OVER,
                    color: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            // Setting this to anything other than Fill requires Features::NON_FILL_POLYGON_MODE
            polygon_mode: wgpu::PolygonMode::Fill,
            // Requires Features::DEPTH_CLIP_CONTROL
            unclipped_depth: false,
            // Requires Features::CONSERVATIVE_RASTERIZATION
            conservative: false,
        },
        depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled,
            depth_compare,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None
    })
}