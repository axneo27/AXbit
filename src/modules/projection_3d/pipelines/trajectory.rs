
use std::borrow::Cow;

use wgpu::util::DeviceExt;
use bytemuck::{Pod, Zeroable};

use crate::modules::projection_3d::window_core::vertex as simple_vertex;

/// Start, End indices (inclusive)
pub(crate) type TrajectorySegment = (u32, u32); 

pub struct DynamicTrajectorySegmentData {
    pub id: u32,
    pub positions: Vec<[f32; 3]>,
    pub segment: TrajectorySegment,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TrajPointRaw {
    position: [f32; 3],
    flag: u32,
}

pub struct TrajectoryPipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,
    traj_buffer: wgpu::Buffer,
    traj_count: u32,
    traj_bind_group_layout: wgpu::BindGroupLayout,
    traj_bind_group: wgpu::BindGroup,
    static_segments: Vec<TrajectorySegment>,
    dynamic_segments: Vec<TrajectorySegment>,
}

impl TrajectoryPipeline {
    pub fn new(
        device: &wgpu::Device,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        positions: &[[f32; 3]],
        flags: &[u32],
    ) -> Self {
        // Build vertex buffer: two vertices per trajectory point with tex_coords.y = +/-1
        let mut vertices: Vec<simple_vertex::Vertex> = Vec::with_capacity(positions.len() * 2);
        for p in positions.iter() {
            vertices.push(simple_vertex::Vertex { position: *p, tex_coords: [0.0, 1.0] });
            vertices.push(simple_vertex::Vertex { position: *p, tex_coords: [0.0, -1.0] });
        }

        let (vertex_buffer, vertex_count) = if vertices.is_empty() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Trajectory Vertex Buffer"),
                size: 1,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            (buffer, 0)
        } else {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Trajectory Vertex Buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            (buffer, vertices.len() as u32)
        };

        // Storage buffer with one entry per trajectory point for tangent computation in shader
        let traj_count = positions.len() as u32;
        let traj_data: Vec<TrajPointRaw> = positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let flag = flags.get(i).copied().unwrap_or(0u32);
                TrajPointRaw { position: [p[0], p[1], p[2]], flag }
            })
            .collect();

        let traj_buffer = if traj_data.is_empty() {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Trajectory Storage Buffer"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Trajectory Storage Buffer"),
                contents: bytemuck::cast_slice(&traj_data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };

        let traj_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Trajectory Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let traj_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Trajectory Bind Group"),
            layout: &traj_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: traj_buffer.as_entire_binding(),
            }],
        });

        let pipeline = Self::create_pipeline(device, camera_bind_group_layout, &traj_bind_group_layout, color_format, depth_format);

        Self {
            pipeline,
            vertex_buffer,
            vertex_count,
            traj_buffer,
            traj_count,
            traj_bind_group_layout,
            traj_bind_group,
            static_segments: Vec::new(),
            dynamic_segments: Vec::new(),
        }
    }

    pub fn update_vertices(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        positions: &[[f32; 3]],
        flags: &[u32],
        positions_wrt_focused: Option<&[DynamicTrajectorySegmentData]>,
    ) {
        let mut active_dynamic_segments = Vec::new();

        // If in integration mode and we have flyby segments for the focused body,
        // replace each matching trajectory segment with positions relative to the focus.
        let positions: Cow<'_, [[f32; 3]]> = if let Some(pos_wrt_focused) = positions_wrt_focused {
            let mut updated_positions = Vec::with_capacity(positions.len());
            updated_positions.extend_from_slice(positions);

            for segment_data in pos_wrt_focused {
                let dyn_segment = segment_data.segment;
                assert!(dyn_segment.1 >= dyn_segment.0, "dynamic trajectory segment end must be >= start");
                let expected_len = (dyn_segment.1 - dyn_segment.0 + 1) as usize;
                assert_eq!(
                    segment_data.positions.len(),
                    expected_len,
                    "focused flyby segment length must match the inclusive dynamic segment range"
                );

                active_dynamic_segments.push(dyn_segment);
                for (offset, pos) in segment_data.positions.iter().enumerate() {
                    let idx = dyn_segment.0 as usize + offset;
                    updated_positions[idx] = *pos;
                }
            }
            Cow::Owned(updated_positions)
        } else {
            Cow::Borrowed(positions)
        };
        
        // Rebuild vertex data (two verts per point)
        let mut vertices: Vec<simple_vertex::Vertex> = Vec::with_capacity(positions.len() * 2);
        for p in positions.iter() {
            vertices.push(simple_vertex::Vertex { position: *p, tex_coords: [0.0, 1.0] });
            vertices.push(simple_vertex::Vertex { position: *p, tex_coords: [0.0, -1.0] });
        }

        let new_vertex_count = vertices.len() as u32;
        let new_traj_count = positions.len() as u32;

        if new_vertex_count == 0 || new_traj_count == 0 {
            self.vertex_count = 0;
            self.traj_count = 0;
            self.static_segments.clear();
            self.dynamic_segments.clear();
            return;
        }

        if new_vertex_count != self.vertex_count {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Trajectory Vertex Buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            self.vertex_buffer = buffer;
            self.vertex_count = new_vertex_count;
        } else {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        }

        // Rebuild storage buffer
        let traj_data: Vec<TrajPointRaw> = positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let flag = flags.get(i).copied().unwrap_or(0u32);
                TrajPointRaw { position: [p[0], p[1], p[2]], flag }
            })
            .collect();

        if new_traj_count != self.traj_count {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Trajectory Storage Buffer"),
                contents: bytemuck::cast_slice(&traj_data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
            self.traj_buffer = buffer;
            self.traj_count = new_traj_count;

            self.traj_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Trajectory Bind Group"),
                layout: &self.traj_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.traj_buffer.as_entire_binding(),
                }],
            });
        } else {
            queue.write_buffer(&self.traj_buffer, 0, bytemuck::cast_slice(&traj_data));
        }

        self.dynamic_segments = active_dynamic_segments;
        self.set_static_segments();
    }

    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, camera_bind_group: &'a wgpu::BindGroup) {
        if self.vertex_count < 4 || self.traj_count < 2 {  
            return;
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, camera_bind_group, &[]);
        render_pass.set_bind_group(1, &self.traj_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));

        if !self.dynamic_segments.is_empty() {
            for &(start, end) in &self.static_segments {
                let vert_start = start * 2;
                let vert_end = (end + 1) * 2;
                if vert_end > vert_start {
                    render_pass.draw(vert_start..vert_end, 0..1);
                }
            }

            for &(start, end) in &self.dynamic_segments {
                let vert_start = start * 2;
                let vert_end = (end + 1) * 2;
                if vert_end > vert_start {
                    render_pass.draw(vert_start..vert_end, 0..1);
                }
            }
        } else {
            render_pass.draw(0..self.vertex_count, 0..1);
        }
    }

    fn create_pipeline(
        device: &wgpu::Device,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        traj_bind_group_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Trajectory Pipeline Layout"),
            bind_group_layouts: &[camera_bind_group_layout, traj_bind_group_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("../window_core/shaders/trajectory.wgsl"));

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Trajectory Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[simple_vertex::Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState {
                        alpha: wgpu::BlendComponent::OVER,
                        color: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
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
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    }

    pub fn set_static_segments(&mut self) {
        self.static_segments.clear();

        if self.dynamic_segments.is_empty() {
            return;
        }

        self.dynamic_segments.sort_unstable_by_key(|segment| segment.0);

        // we do this in case there are multiple dynamic segments that are overlapping
        // idk when this would happen, but we have to merge them
        let mut merged_dynamic_segments: Vec<TrajectorySegment> =
            Vec::with_capacity(self.dynamic_segments.len());
        for (start, end) in self.dynamic_segments.iter().copied() {
            if start > end || end >= self.traj_count {
                continue;
            }

            if let Some(last) = merged_dynamic_segments.last_mut()
                && start <= last.1.saturating_add(1) {
                last.1 = last.1.max(end);
            } else {
                merged_dynamic_segments.push((start, end));
            }
        }

        self.dynamic_segments = merged_dynamic_segments;

        let mut cursor = 0u32;
        for &(start, end) in &self.dynamic_segments {
            if cursor < start {
                self.static_segments.push((cursor, start - 1));
            }
            cursor = end.saturating_add(1);
        }

        if cursor < self.traj_count {
            self.static_segments.push((cursor, self.traj_count - 1));
        }
    }
}
