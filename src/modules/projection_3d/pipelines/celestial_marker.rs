use crate::modules::projection_3d::pipelines::common::create_quad_mesh_data;
use crate::modules::projection_3d::pipelines::indirect::IndirectResources;
use crate::modules::projection_3d::window_core::model::{ModelVertex, Model, Material, Vertex};
use crate::modules::projection_3d::window_core::instance::{Instance, InstanceRawWithoutNormal};
use crate::modules::projection_3d::simulation::Simulation;
use wgpu::util::DeviceExt;

/// 16777215
const MAX_PICKABLE_INSTANCES: usize = 0xFF_FFFF;

fn decode_picking_index(encoded: u32) -> Option<usize> {
    encoded.checked_sub(1).map(|index| index as usize)
}

pub struct CelestialMarkerPipeline {
    model: Model,
    pipeline: wgpu::RenderPipeline,
    instance_count: u32,
    instance_buffer: wgpu::Buffer,

    picking_pipeline: wgpu::RenderPipeline,
    picking_texture: wgpu::Texture,
    picking_view: wgpu::TextureView,
    picking_staging_buffer: wgpu::Buffer,

    visible_ids: Vec<i32>,
    // Mirrors the uploaded instance buffer as (object ID, is SBDB). Keeping this
    // snapshot here guarantees that a picked index uses the same instance order.
    picking_targets: Vec<(Option<i32>, bool)>,

    indirect: IndirectResources
}

impl CelestialMarkerPipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        color_rgba: [u8; 4],
        instances: &[Instance],
        picking_size: wgpu::Extent3d,
        visible_ids: Vec<i32>
    ) -> Self {
        assert!(instances.len() <= MAX_PICKABLE_INSTANCES, "Picking supports at most 16,777,215 marker instances");

        let (model, vc) = Self::create_celestial_marker_model(device, queue, texture_bind_group_layout, color_rgba);

        // create the render bind group layout early so the render pipeline
        // can include it (shader expects @group(1) resources).
        let render_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Render indirect instances"),
            entries: &[
                // binding 0 – all_instances
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 1 – visible_indices
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline = Self::create_celestial_marker_pipeline(device, camera_bind_group_layout, &render_bind_group_layout, color_format, depth_format);
        let instance_data = instances.iter().map(Instance::to_raw_without_normal).collect::<Vec<InstanceRawWithoutNormal>>();
        let instance_buffer = if instance_data.is_empty() {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Celestial Marker Instance Buffer"),
                size: std::mem::size_of::<InstanceRawWithoutNormal>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Celestial Marker Instance Buffer"),
                contents: bytemuck::cast_slice(&instance_data),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };

        let picking_pipeline = Self::create_celestial_picking_pipeline(device, camera_bind_group_layout, &render_bind_group_layout, depth_format);
        let picking_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Picking Texture"),
            size: picking_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let picking_view = picking_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let picking_staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Picking Staging Buffer"),
            size: 256,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });



        // use shared helper to create indirect/visibility resources (compute + bind-groups)
        let indirect = IndirectResources::new(
            device,
            &instance_buffer,
            instances.len(),
            vc,
            &render_bind_group_layout,
            "Celestial",
        );

        Self {
            model,
            pipeline,
            instance_count: instances.len() as u32,
            instance_buffer,

            picking_pipeline,
            picking_texture,
            picking_staging_buffer,
            picking_view,

            visible_ids,
            picking_targets: instances.iter().map(|instance| (instance.id, instance.is_sbdb)).collect(),
            indirect,
        }
    }

    pub fn update_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance], paused: bool, focused_changed: bool) {
        // Only skip writes when paused _and_ the focus hasn't changed. If the
        // focused body changed we must always update instance buffers so the
        // visible set / transforms reflect the new focus.
        if paused && !focused_changed {
            return;
        }

        assert!(instances.len() <= MAX_PICKABLE_INSTANCES, "Picking supports at most 16,777,215 marker instances");
        let new_count = instances.len() as u32;

        if new_count != self.instance_count {
            self.instance_count = new_count;

            self.indirect.resize(device, new_count as usize);

            if new_count == 0 {
                self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Celestial Marker Instance Buffer (empty)"),
                    size: 4,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });

                self.indirect.update_instance_buffer(device, &self.instance_buffer);
            } else {
                let instance_data = instances.iter().map(Instance::to_raw_without_normal).collect::<Vec<InstanceRawWithoutNormal>>();
                self.instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Celestial Marker Instance Buffer"),
                    contents: bytemuck::cast_slice(&instance_data),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                });

                // rebind newly-created instance buffer into indirect resources
                self.indirect.update_instance_buffer(device, &self.instance_buffer);
            }
        } else if new_count > 0 {
            let instance_data = instances.iter().map(Instance::to_raw_without_normal).collect::<Vec<InstanceRawWithoutNormal>>();
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instance_data));
        }

        self.picking_targets = instances.iter().map(|instance| (instance.id, instance.is_sbdb)).collect();
    }

    pub fn draw_celestial_marker_model<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, camera_bind_group: &'a wgpu::BindGroup) {
        if self.instance_count == 0 {
            return;
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, camera_bind_group, &[]);
        render_pass.set_bind_group(1, self.indirect.render_bind_group(), &[]);

        render_pass.set_vertex_buffer(0, self.model.meshes[0].vertex_buffer.slice(..));
        render_pass.set_index_buffer(self.model.meshes[0].index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        render_pass.draw_indexed_indirect(self.indirect.indirect_buffer(), 0);
    }

    fn create_celestial_marker_model(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        color_rgba: [u8; 4],
    ) -> (Model, u32) {
        let mesh_data = create_quad_mesh_data();
        let mesh = super::common::create_mesh(device, &mesh_data, "celestial_marker", 0);
        
        let index_count = mesh_data.indices.len() as u32;

        let diffuse = super::common::create_solid_texture(device, queue, color_rgba, "celestial_marker_diffuse", false);
        let normal_flat = super::common::create_solid_texture(device, queue, [128, 128, 255, 255], "celestial_marker_normal", true);

        let material = Material::new(device, "celestial_marker_material", diffuse, normal_flat, layout);

        (Model { meshes: vec![mesh], materials: vec![material] }, index_count)
    }

    fn create_celestial_marker_pipeline(
        device: &wgpu::Device,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        render_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
    ) -> wgpu::RenderPipeline {
        // pipeline needs both group(0) camera and group(1) instance/indices layouts
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Celestial Marker Pipeline Layout"),
            bind_group_layouts: &[camera_bind_group_layout, render_bind_group_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("../window_core/shaders/cel_marker.wgsl"));

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Celestial Marker Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // instance data is sourced from storage (visible_indices + all_instances)
                buffers: &[ModelVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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

    fn create_celestial_picking_pipeline(
        device: &wgpu::Device,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        render_bind_group_layout: &wgpu::BindGroupLayout,
        depth_format: Option<wgpu::TextureFormat>,
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Celestial Marker Picking Pipeline Layout"),
            bind_group_layouts: &[camera_bind_group_layout, render_bind_group_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("../window_core/shaders/cel_picking.wgsl"));

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Celestial Marker Picking Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // instance data is sourced from storage (visible_indices + all_instances)
                buffers: &[ModelVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
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
            // enable depth testing to match visual output (don't modify depth)
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        })
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {

        self.picking_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Picking Texture"),
            size: wgpu::Extent3d {
                width, height, depth_or_array_layers: 1
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        // Update the view to point to the newly created texture
        self.picking_view = self.picking_texture.create_view(&wgpu::TextureViewDescriptor::default());
    }

    pub fn perform_picking_pass<'a>(&self, encoder: &mut wgpu::CommandEncoder, camera_bind_group: &'a wgpu::BindGroup, depth_view: &'a wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Celestial Picking Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.picking_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(&self.picking_pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_bind_group(1, self.indirect.render_bind_group(), &[]);
        pass.set_vertex_buffer(0, self.model.meshes[0].vertex_buffer.slice(..));
        pass.set_index_buffer(self.model.meshes[0].index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        // draw only visible instances using the same indirect args used by the main renderer
        pass.draw_indexed_indirect(self.indirect.indirect_buffer(), 0);
    }

    pub async fn start_async_picking_readback(
        &mut self, 
        mouse_x: f64, 
        mouse_y: f64, 
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        simulation: &mut Simulation,
    ) {

        // Use a dedicated encoder for the copy so we own it here.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Picking Copy Encoder"),
        });

        let size = self.picking_texture.size();
        let tex_w = size.width as f64;
        let tex_h = size.height as f64;

        // Mouse coordinates are already in physical pixels; clamp to [0, w-1] / [0, h-1].
        let origin_x = mouse_x.clamp(0.0, (tex_w - 1.0).max(0.0)) as u32;
        let origin_y = mouse_y.clamp(0.0, (tex_h - 1.0).max(0.0)) as u32;

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.picking_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin_x,
                    y: origin_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.picking_staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),         
                    rows_per_image: None,             
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );

        queue.submit(std::iter::once(encoder.finish()));

        {
            let buffer_slice = self.picking_staging_buffer.slice(..);
            let (tx, rx) = futures_intrusive::channel::shared::oneshot_channel();

            buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
                if result.is_ok() {
                    tx.send(result).ok();
                }
            });
            let _ = device.poll(wgpu::PollType::wait_indefinitely()); 
            rx.receive().await.unwrap().unwrap();
            
            let data = buffer_slice.get_mapped_range();
            let mut pixel_bytes = [0u8; 4];
            pixel_bytes.copy_from_slice(&data[0..4]);

            let r = pixel_bytes[0] as u32;
            let g = pixel_bytes[1] as u32;
            let b = pixel_bytes[2] as u32;

            let encoded = r | (g << 8) | (b << 16);

            // Encoded value 0 is background; value 1 maps to array index 0.
            // Because when we get 0, decode_picking_index returns None because of checked_sub(1).
            if let Some(index) = decode_picking_index(encoded) {
                if let Some(&(Some(id), is_sbdb)) = self.picking_targets.get(index) {
                    let exists = if is_sbdb {
                        simulation.sb_celestial_objects.contains_key(&id)
                    } else {
                        simulation.naif_celestial_objects.contains_key(&id)
                    };

                    if exists {
                        simulation.focused_body_id = id;
                        simulation.focused_body_type = if is_sbdb {
                            crate::modules::projection_3d::simulation::FocusedBodyType::SmallBody
                        } else {
                            crate::modules::projection_3d::simulation::FocusedBodyType::Naif
                        };
                    }
                }
            }

            drop(data);
        }
        self.picking_staging_buffer.unmap();
        

    }

    pub fn update_visibility(&mut self, queue: &wgpu::Queue, visible_ids: &Vec<i32>, instances: &[Instance]) {
        // Update the compact visibility-staging buffer (0/1 per instance).
        let mut flags = vec![0u32; instances.len()];
        for (i, inst) in instances.iter().enumerate() {
            if let Some(id) = inst.id {
                if visible_ids.contains(&id) {
                    flags[i] = 1u32;
                }
            }
        }
        self.indirect.set_visibility_flags(queue, &flags);

        // keep local copy for diagnostics / future use
        self.visible_ids = visible_ids.clone();

        // ensure counter is zeroed on CPU side when there are no instances
        if instances.is_empty() {
            self.indirect.reset_visible_count(queue);
        }
    }

    pub fn generate_indirect(&self, encoder: &mut wgpu::CommandEncoder) {
        self.indirect.generate_indirect(encoder);
    }

}

#[cfg(test)]
mod tests {
    use super::decode_picking_index;

    #[test]
    fn picking_index_reserves_zero_for_background() {
        assert_eq!(decode_picking_index(0), None);
        assert_eq!(decode_picking_index(1), Some(0));
        assert_eq!(decode_picking_index(0xFF_FFFF), Some(0xFF_FFFE));
    }
}
