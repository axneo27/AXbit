use wgpu::util::DeviceExt;

use crate::modules::projection_3d::pipelines::common::DrawParams;

pub struct IndirectResources {
    pub instance_count: u32,

    pub indirect_buffer: wgpu::Buffer,
    pub visibility_buffer: wgpu::Buffer,
    pub visible_indices_buffer: wgpu::Buffer,
    pub visible_count_buffer: wgpu::Buffer,
    pub draw_params_buffer: wgpu::Buffer,

    pub compute_pipeline: wgpu::ComputePipeline,
    pub compute_pipeline_finalize: wgpu::ComputePipeline,
    pub compute_bind_group: wgpu::BindGroup,

    pub render_bind_group_layout: wgpu::BindGroupLayout,
    pub render_bind_group: wgpu::BindGroup,

    // keep the compute bind-group-layout so we can recreate bind-groups
    compute_bind_group_layout: wgpu::BindGroupLayout,
}

impl IndirectResources {
    /// Returns a `BindGroupLayout` suitable to be included into a render pipeline
    /// layout so the vertex shader can read `all_instances` + `visible_indices`.
    pub fn render_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render indirect instances layout"),
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
        })
    }

    /// Create all GPU-side buffers/pipelines used to build indirect args from
    /// a `visibility` bitset. `instance_buffer` is *not* owned by this helper —
    /// it must remain valid for the lifetime of the returned object and can be
    /// re-bound later via `update_instance_buffer`.
    pub fn new(
        device: &wgpu::Device,
        instance_buffer: &wgpu::Buffer,
        instance_count: usize,
        index_count: u32,
        render_bind_group_layout: &wgpu::BindGroupLayout,
        label_prefix: &str,
    ) -> Self {
        // indirect / visibility buffers
        let indirect_size = std::mem::size_of::<wgpu::util::DrawIndexedIndirectArgs>() as u64;
        let indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{} Indirect Buffer", label_prefix)),
            size: indirect_size,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let visibility_flags = vec![0u32; instance_count.max(1)];
        let visibility_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} Visibility Buffer", label_prefix)),
            contents: bytemuck::cast_slice(&visibility_flags),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let visible_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{} Visible Indices Buffer", label_prefix)),
            size: (instance_count.max(1) * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        let visible_count_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} Visible Count Buffer", label_prefix)),
            contents: bytemuck::cast_slice(&[0u32]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let draw_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} Draw params", label_prefix)),
            contents: bytemuck::cast_slice(&[DrawParams::new(index_count)]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // compute shader for culling/indirect args (reuse existing WGSL)
        let module = device.create_shader_module(wgpu::include_wgsl!("../window_core/shaders/indirect_args_wn.wgsl"));
        let compute_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{} compute cull layout", label_prefix)),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: Some(std::num::NonZeroU64::new(std::mem::size_of::<DrawParams>() as u64).unwrap()) }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: Some(std::num::NonZeroU64::new(4).unwrap()) }, count: None },
            ],
        });

        let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&compute_bind_group_layout], push_constant_ranges: &[] });
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(&format!("{} indirect compute", label_prefix)), layout: Some(&compute_layout), module: &module, entry_point: Some("generate_indirect"), compilation_options: Default::default(), cache: None });
        let compute_pipeline_finalize = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(&format!("{} indirect finalize", label_prefix)), layout: Some(&compute_layout), module: &module, entry_point: Some("finalize_indirect"), compilation_options: Default::default(), cache: None });

        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(&format!("{} compute bind group", label_prefix)), layout: &compute_bind_group_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: instance_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: visibility_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: visible_indices_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: indirect_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: draw_params_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: visible_count_buffer.as_entire_binding() },
        ] });

        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(&format!("{} render bind group", label_prefix)), layout: render_bind_group_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: instance_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: visible_indices_buffer.as_entire_binding() },
        ] });

        IndirectResources {
            instance_count: instance_count as u32,
            indirect_buffer,
            visibility_buffer,
            visible_indices_buffer,
            visible_count_buffer,
            draw_params_buffer,
            compute_pipeline,
            compute_pipeline_finalize,
            compute_bind_group,
            render_bind_group_layout: render_bind_group_layout.clone(),
            render_bind_group,
            compute_bind_group_layout,
        }
    }

    /// Replace the instance buffer binding (useful when the pipeline recreates
    /// a new instance buffer because the instance count changed).
    pub fn update_instance_buffer(&mut self, device: &wgpu::Device, instance_buffer: &wgpu::Buffer) {
        // recreate compute & render bind groups with the new instance buffer
        self.compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("compute bind group (rebind)"), layout: &self.compute_bind_group_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: instance_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: self.visibility_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: self.visible_indices_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: self.indirect_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: self.draw_params_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: self.visible_count_buffer.as_entire_binding() },
        ] });

        self.render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("render bind group (rebind)"), layout: &self.render_bind_group_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: instance_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: self.visible_indices_buffer.as_entire_binding() },
        ] });
    }

    /// Resize internal visibility/indices buffers when the instance count
    /// changes.
    pub fn resize(&mut self, device: &wgpu::Device, new_instance_count: usize) {
        self.instance_count = new_instance_count as u32;

        // recreate buffers sized for new_instance_count
        let visibility_flags = vec![0u32; new_instance_count.max(1)];
        self.visibility_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("Visibility Buffer (resized)"), contents: bytemuck::cast_slice(&visibility_flags), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST });

        self.visible_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("Visible Indices Buffer (resized)"), size: (new_instance_count.max(1) * std::mem::size_of::<u32>()) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX, mapped_at_creation: false });

        self.visible_count_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("Visible Count Buffer (resized)"), contents: bytemuck::cast_slice(&[0u32]), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST });
    }

    /// Write a 0/1 visibility flags array into the GPU `visibility_buffer`.
    pub fn set_visibility_flags(&self, queue: &wgpu::Queue, flags: &[u32]) {
        queue.write_buffer(&self.visibility_buffer, 0, bytemuck::cast_slice(flags));
    }

    /// Reset the atomic visible-count (useful when CPU decides there are no instances).
    pub fn reset_visible_count(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.visible_count_buffer, 0, bytemuck::cast_slice(&[0u32]));
    }

    /// Dispatch the compute pass that generates the indirect args from the
    /// visibility buffer.
    pub fn generate_indirect(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.instance_count == 0 { return; }
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Indirect Cull Pass"), timestamp_writes: None });
        cpass.set_pipeline(&self.compute_pipeline);
        cpass.set_bind_group(0, &self.compute_bind_group, &[]);
        cpass.dispatch_workgroups((self.instance_count + 63) / 64, 1, 1);
        cpass.set_pipeline(&self.compute_pipeline_finalize);
        cpass.dispatch_workgroups(1, 1, 1);
    }

    pub fn render_bind_group(&self) -> &wgpu::BindGroup { &self.render_bind_group }
    pub fn indirect_buffer(&self) -> &wgpu::Buffer { &self.indirect_buffer }
}
