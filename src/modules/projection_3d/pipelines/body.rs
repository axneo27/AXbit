use crate::modules::projection_3d::pipelines::indirect::IndirectResources;
use crate::modules::projection_3d::window_core::model::{ModelVertex, Model, Vertex};
use crate::modules::projection_3d::window_core::instance::{Instance, InstanceRaw};
use wgpu::util::DeviceExt;
use std::sync::Arc;
use cgmath::{Matrix, SquareMatrix};

pub struct BodyPipeline {
    pub model: Arc<Model>,
    pub pipeline: wgpu::RenderPipeline,
    pub instance_count: u32,
    pub instance_buffer: wgpu::Buffer,

    // storage-backed instance buffer (used by GPU-driven rendering)
    instance_storage_buffer: wgpu::Buffer,

    // indirect / compute resources
    indirect: IndirectResources,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BodyInstanceStorage {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 4],
    color: [f32; 4],
    id: [u32; 4],
}

impl BodyPipeline {
    pub fn new(
        device: &wgpu::Device,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        light_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        model: Arc<Model>,
        instances: &[Instance],
    ) -> Self {
        // Create render bind layout for storage-driven instances
        let render_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Body render bind group"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });

        let pipeline = Self::create_body_pipeline(device, camera_bind_group_layout, light_bind_group_layout, &render_bind_group_layout, color_format, depth_format);

        // vertex instance buffer (kept for compatibility with other code paths)
        let instance_data = instances.iter().map(Instance::to_raw).collect::<Vec<InstanceRaw>>();
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Body Instance Buffer"),
            contents: bytemuck::cast_slice(&instance_data),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        // storage-backed instance buffer (packed for WGSL storage layout)
        let mut storage_instances = Vec::with_capacity(instances.len());
        for inst in instances.iter() {
            let translation = cgmath::Matrix4::from_translation(inst.position);
            let rotation = cgmath::Matrix4::from(inst.rotation);
            let scale = cgmath::Matrix4::from_nonuniform_scale(inst.scale.x, inst.scale.y, inst.scale.z);
            let model_mat = translation * rotation * scale;

            let model4: [[f32;4];4] = model_mat.into();

            let model_3x3 = cgmath::Matrix3::new(
                model_mat.x.x, model_mat.x.y, model_mat.x.z,
                model_mat.y.x, model_mat.y.y, model_mat.y.z,
                model_mat.z.x, model_mat.z.y, model_mat.z.z,
            );

            let normal_matrix = model_3x3.invert().map(|inv| inv.transpose()).unwrap_or(cgmath::Matrix3::identity());
            let normal3: [[f32;3];3] = normal_matrix.into();
            let normal4 = [
                [normal3[0][0], normal3[0][1], normal3[0][2], 0.0],
                [normal3[1][0], normal3[1][1], normal3[1][2], 0.0],
                [normal3[2][0], normal3[2][1], normal3[2][2], 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ];

            let color4 = [inst.color[0], inst.color[1], inst.color[2], 1.0f32];
            let id4 = [inst.id.unwrap_or(0) as u32, 0u32, 0u32, 0u32];
            storage_instances.push(BodyInstanceStorage { model: model4, normal: normal4, color: color4, id: id4 });
        }
        let instance_storage_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Body Instance Storage Buffer"),
            contents: bytemuck::cast_slice(&storage_instances),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Create shared indirect/visibility resources bound to the storage-backed
        // instance buffer (Body's shaders read instances from storage).
        let indirect = IndirectResources::new(
            device,
            &instance_storage_buffer,
            instances.len(),
            model.meshes[0].num_elements,
            &render_bind_group_layout,
            "Body",
        );

        Self {
            model,
            pipeline,
            instance_count: instances.len() as u32,
            instance_buffer,
            instance_storage_buffer,
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

            if new_count == 0 {
                self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Body Instance Buffer (empty)"),
                    size: 1,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });

                self.instance_storage_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Body Instance Storage Buffer (empty)"),
                    size: 4,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            } else {
                let instance_data = instances.iter().map(Instance::to_raw).collect::<Vec<InstanceRaw>>();
                self.instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Body Instance Buffer"),
                    contents: bytemuck::cast_slice(&instance_data),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });

                // recreate storage buffer with packed BodyInstanceStorage
                let mut storage_instances = Vec::with_capacity(instances.len());
                for inst in instances.iter() {
                    let translation = cgmath::Matrix4::from_translation(inst.position);
                    let rotation = cgmath::Matrix4::from(inst.rotation);
                    let scale = cgmath::Matrix4::from_nonuniform_scale(inst.scale.x, inst.scale.y, inst.scale.z);
                    let model_mat = translation * rotation * scale;
                    let model4: [[f32;4];4] = model_mat.into();

                    let model_3x3 = cgmath::Matrix3::new(
                        model_mat.x.x, model_mat.x.y, model_mat.x.z,
                        model_mat.y.x, model_mat.y.y, model_mat.y.z,
                        model_mat.z.x, model_mat.z.y, model_mat.z.z,
                    );
                    let normal_matrix = model_3x3.invert().map(|inv| inv.transpose()).unwrap_or(cgmath::Matrix3::identity());
                    let normal3: [[f32;3];3] = normal_matrix.into();
                    let normal4 = [
                        [normal3[0][0], normal3[0][1], normal3[0][2], 0.0],
                        [normal3[1][0], normal3[1][1], normal3[1][2], 0.0],
                        [normal3[2][0], normal3[2][1], normal3[2][2], 0.0],
                        [0.0, 0.0, 0.0, 1.0],
                    ];
                    let color4 = [inst.color[0], inst.color[1], inst.color[2], 1.0f32];
                    let id4 = [inst.id.unwrap_or(0) as u32, 0u32, 0u32, 0u32];
                    storage_instances.push(BodyInstanceStorage { model: model4, normal: normal4, color: color4, id: id4 });
                }
                self.instance_storage_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Body Instance Storage Buffer"),
                    contents: bytemuck::cast_slice(&storage_instances),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                });
            }

            // resize & rebind indirect resources to match the new instance count
            self.indirect.resize(device, new_count as usize);
            self.indirect.update_instance_buffer(device, &self.instance_storage_buffer);
        } else if new_count > 0 {
            let instance_data = instances.iter().map(Instance::to_raw).collect::<Vec<InstanceRaw>>();
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instance_data));

            // update storage buffer
            let mut storage_instances = Vec::with_capacity(instances.len());
            for inst in instances.iter() {
                let translation = cgmath::Matrix4::from_translation(inst.position);
                let rotation = cgmath::Matrix4::from(inst.rotation);
                let scale = cgmath::Matrix4::from_nonuniform_scale(inst.scale.x, inst.scale.y, inst.scale.z);
                let model_mat = translation * rotation * scale;
                let model4: [[f32;4];4] = model_mat.into();

                let model_3x3 = cgmath::Matrix3::new(
                    model_mat.x.x, model_mat.x.y, model_mat.x.z,
                    model_mat.y.x, model_mat.y.y, model_mat.y.z,
                    model_mat.z.x, model_mat.z.y, model_mat.z.z,
                );
                let normal_matrix = model_3x3.invert().map(|inv| inv.transpose()).unwrap_or(cgmath::Matrix3::identity());
                let normal3: [[f32;3];3] = normal_matrix.into();
                let normal4 = [
                    [normal3[0][0], normal3[0][1], normal3[0][2], 0.0],
                    [normal3[1][0], normal3[1][1], normal3[1][2], 0.0],
                    [normal3[2][0], normal3[2][1], normal3[2][2], 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ];
                let color4 = [inst.color[0], inst.color[1], inst.color[2], 1.0f32];
                let id4 = [inst.id.unwrap_or(0) as u32, 0u32, 0u32, 0u32];
                storage_instances.push(BodyInstanceStorage { model: model4, normal: normal4, color: color4, id: id4 });
            }
            queue.write_buffer(&self.instance_storage_buffer, 0, bytemuck::cast_slice(&storage_instances));
        }
    }

    pub fn draw_body_model<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, camera_bind_group: &'a wgpu::BindGroup, light_bind_group: &'a wgpu::BindGroup) {
        if self.instance_count == 0 {
            return;
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, camera_bind_group, &[]);
        render_pass.set_bind_group(1, light_bind_group, &[]);
        // render bind-group (group 2) contains storage-backed instances + visible indices
        render_pass.set_bind_group(2, self.indirect.render_bind_group(), &[]);

        render_pass.set_vertex_buffer(0, self.model.meshes[0].vertex_buffer.slice(..));
        render_pass.set_index_buffer(self.model.meshes[0].index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        // indirect draw: compute pass writes indirect args into `indirect_buffer`
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

    fn create_body_pipeline(
        device: &wgpu::Device,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        light_bind_group_layout: &wgpu::BindGroupLayout,
        render_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Body Pipeline Layout"),
            bind_group_layouts: &[camera_bind_group_layout, light_bind_group_layout, render_bind_group_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("../window_core/shaders/colored_body.wgsl"));

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Body Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // instance data is read from storage buffer in WGSL
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
                cull_mode: Some(wgpu::Face::Back),
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