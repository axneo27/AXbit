use std::f32::consts::TAU;
use wgpu::util::DeviceExt;

use crate::modules::projection_3d::pipelines::common::{MeshData, create_mesh, create_solid_texture};
use crate::modules::projection_3d::pipelines::indirect::IndirectResources;
use crate::modules::projection_3d::window_core::model::{ModelVertex, Model, Material, Vertex};
use crate::modules::projection_3d::window_core::instance::{Instance, InstanceRawWithoutNormal};
pub struct OrbitPipeline {
    pub model: Model,
    pub pipeline: wgpu::RenderPipeline,
    pub instance_count: u32,
    pub instance_buffer: wgpu::Buffer,

    // GPU-driven visibility / indirect draw resources
    indirect: IndirectResources,
}

impl OrbitPipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        segment_count: u32,
        color_rgba: [u8; 4],
        instances: &[Instance],
    ) -> Self {
        let mesh_data = generate_unit_circle_strip(segment_count);
        let mesh = create_mesh(device, &mesh_data, "orbit_ring", 0);
        let model = Model { meshes: vec![mesh], materials: vec![Material::new(device, "orbit_material", create_solid_texture(device, queue, color_rgba, "orbit_diffuse", false), create_solid_texture(device, queue, [128,128,255,255], "orbit_normal", true), texture_bind_group_layout)] };
        let index_count = mesh_data.indices.len() as u32;

        let render_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Orbit render bind group"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ] });

        let pipeline = Self::create_orbit_pipeline(device, camera_bind_group_layout, &render_bind_group_layout, color_format, depth_format);

        let instance_data = instances.iter().map(Instance::to_raw_without_normal).collect::<Vec<InstanceRawWithoutNormal>>();
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Orbit Instance Buffer"),
            contents: bytemuck::cast_slice(&instance_data),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let render_bind_group_layout = IndirectResources::render_bind_group_layout(device);
        let indirect = IndirectResources::new(device, &instance_buffer, instances.len(), index_count, &render_bind_group_layout, "Orbit");

        Self {
            model,
            pipeline,
            instance_count: instances.len() as u32,
            instance_buffer,
            indirect,
        }
    }

    pub fn update_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance], paused: bool, focused_changed: bool) {
        if paused && !focused_changed {
            return;
        }

        let new_count = instances.len() as u32;

        if new_count != self.instance_count {
            self.instance_count = new_count;

            self.indirect.resize(device, new_count as usize);

            if new_count == 0 {
                self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Orbit Instance Buffer (empty)"),
                    size: 4,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });

                self.indirect.update_instance_buffer(device, &self.instance_buffer);
            } else {
                let instance_data = instances.iter().map(Instance::to_raw_without_normal).collect::<Vec<InstanceRawWithoutNormal>>();
                
                self.instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Orbit Instance Buffer"),
                    contents: bytemuck::cast_slice(&instance_data),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                });

                self.indirect.update_instance_buffer(device, &self.instance_buffer);
            }
        } else if new_count > 0 {

            let instance_data = instances.iter().map(Instance::to_raw_without_normal).collect::<Vec<InstanceRawWithoutNormal>>();
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instance_data));
        }
    }

    pub fn draw_orbit_model<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, camera_bind_group: &'a wgpu::BindGroup) {
        if self.instance_count == 0 {
            return;
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, camera_bind_group, &[]);
        render_pass.set_bind_group(1, self.indirect.render_bind_group(), &[]);

        render_pass.set_vertex_buffer(0, self.model.meshes[0].vertex_buffer.slice(..));
        render_pass.set_index_buffer(self.model.meshes[0].index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        // use indirect args generated by compute pass
        render_pass.draw_indexed_indirect(self.indirect.indirect_buffer(), 0);
    }

    pub fn update_visibility(&mut self, queue: &wgpu::Queue, visible_ids: &Vec<i32>, instances: &[Instance]) {
        let mut flags = vec![0u32; instances.len()];
        for (i, inst) in instances.iter().enumerate() {
            if let Some(id) = inst.id {
                if visible_ids.contains(&id) {
                    flags[i] = 1u32;
                }
            }
        }
        self.indirect.set_visibility_flags(queue, &flags);
    }

    pub fn generate_indirect(&self, encoder: &mut wgpu::CommandEncoder) {
        self.indirect.generate_indirect(encoder);
    }

    // fn create_orbit_model(
    //     device: &wgpu::Device,
    //     queue: &wgpu::Queue,
    //     layout: &wgpu::BindGroupLayout,
    //     segment_count: u32,
    //     color_rgba: [u8; 4],
    // ) -> Model {
    //     let mesh_data = generate_unit_circle_strip(segment_count);
    //     let mesh = create_mesh(device, &mesh_data, "orbit_ring", 0);

    //     let diffuse = create_solid_texture(device, queue, color_rgba, "orbit_diffuse", false);
    //     let normal_flat = create_solid_texture(device, queue, [128, 128, 255, 255], "orbit_normal", true);

    //     let material = Material::new(device, "orbit_material", diffuse, normal_flat, layout);

    //     Model { meshes: vec![mesh], materials: vec![material] }
    // }

    fn create_orbit_pipeline(
        device: &wgpu::Device,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        render_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Orbit Pipeline Layout"),
            bind_group_layouts: &[camera_bind_group_layout, render_bind_group_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("../window_core/shaders/orbit.wgsl"));

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Orbit Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[ModelVertex::desc()],
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
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        })
    }
}

// Generate a unit circle strip: for each segment, two identical vertices
// with tex_coords.y = +1.0 and -1.0 indicating extrusion direction.
pub fn generate_unit_circle_strip(segment_count: u32) -> MeshData {
    let n = segment_count.max(32);
    let mut vertices = Vec::with_capacity((n as usize) * 2);

    for i in 0..n {
        let t = i as f32 / n as f32;
        let a = TAU * t;
        let (s, c) = a.sin_cos();
        let pos = [c, s, 0.0];

        // direction +1
        vertices.push(ModelVertex {
            position: pos,
            tex_coords: [t, 1.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        });
        // direction -1
        vertices.push(ModelVertex {
            position: pos,
            tex_coords: [t, -1.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        });
    }

    let mut indices = Vec::with_capacity((n as usize) * 6);
    for i in 0..n {
        let i_pos = i * 2;           // +1 dir
        let i_neg = i_pos + 1;       // -1 dir
        let i_pos_next = ((i + 1) % n) * 2; // next +1
        let i_neg_next = i_pos_next + 1;    // next -1

        indices.push(i_pos as u32);
        indices.push(i_pos_next as u32);
        indices.push(i_neg as u32);

        indices.push(i_neg as u32);
        indices.push(i_pos_next as u32);
        indices.push(i_neg_next as u32);
    }

    MeshData { vertices, indices }
}




