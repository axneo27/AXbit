use std::{iter, sync::{Arc, mpsc}};
use anyhow::Result;
use wgpu::util::DeviceExt;
use chrono::{NaiveDateTime, Datelike, Timelike, Duration};

use winit::{
    event::*, event_loop::ActiveEventLoop, keyboard::KeyCode, window::{Window}
};

use crate::modules::{projection_3d::{pipelines::common::create_render_pipeline_default, simulation, state::Vec3d, window_core::{camera::{self, Camera, CameraController, CameraUniform, Projection}, hdr::HdrPipeline, light::LightUniform, model::{self, DrawLight, Model, Vertex}, texture::{self, Texture}}}, utils};
use crate::modules::spice_ker::{self, GroupInfoShort};
use super::super::super::pipelines::{orbit, celestial_marker, closest_approach, body, trajectory};

use crate::modules::projection_3d::traj::ProximityReference;
use simulation::{Simulation, CoordSystem};
use super::egui_tools::EguiRenderer;
use super::kernel_setup::KernelStartupSelection;
use super::ui;
use egui_wgpu::ScreenDescriptor;

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    viewport_size: [f32; 2],
}

impl Uniforms {
    pub fn new(viewport_size: [f32; 2]) -> Self {
        Self { viewport_size }
    }
}
use crate::modules::projection_3d::window_core::resources;

pub struct AppState {
    surface: wgpu::Surface<'static>,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,
    camera: Camera,
    camera_uniform: CameraUniform,

    projection: Projection,
    camera_buffer: wgpu::Buffer,
    uniforms: Uniforms,
    uniforms_buffer: wgpu::Buffer,
    pub(crate) camera_controller: CameraController,
    camera_bind_group: wgpu::BindGroup,

    depth_texture: Texture,

    pub(crate) orbit_pipeline: orbit::OrbitPipeline,
    pub(crate) celestial_marker_pipeline: celestial_marker::CelestialMarkerPipeline,
    pub(crate) closest_approach_pipeline: closest_approach::ClosestApproachPipeline,
    pub(crate) body_pipeline: body::BodyPipeline,
    pub(crate) trajectory_pipeline: trajectory::TrajectoryPipeline,

    hdr: HdrPipeline,
    environment_bind_group: wgpu::BindGroup,
    sky_pipeline: wgpu::RenderPipeline,

    #[allow(dead_code)]
    sun_light_uniform: LightUniform,    
    #[allow(dead_code)]
    sun_light_buffer: wgpu::Buffer,
    #[allow(dead_code)]
    sun_light_bind_group: wgpu::BindGroup,
    sun_light_render_pipeline: wgpu::RenderPipeline,

    sphere_model: Arc<Model>,
    pub(crate) simulation: Simulation,
    pub(crate) render_origin: Vec3d,
    pub(crate) window: Arc<Window>,

    pub(crate) right_pressed: bool,
    pub(crate) mouse_x: f64, 
    pub(crate) mouse_y: f64,
    need_celestial_picking: bool,

    cached_focused_id: Option<i32>,
    cached_target_pos: cgmath::Point3<f32>,
    cached_min_distance: f32,
    cached_show_barycenters: bool,
    cached_sim_time_et: f64,
    pub(crate) cached_trajectory_flyby: Option<(i32, Vec<trajectory::DynamicTrajectorySegmentData>)>,
    pub(crate) set_year: i32,
    pub(crate) set_month: u32,
    pub(crate) set_day: u32,
    pub(crate) set_hour: u32,
    pub(crate) set_minute: u32,
    pub(crate) set_second: u32,
    pub(crate) time_error: Option<String>,
    pub(crate) egui_renderer: EguiRenderer,

    // FPS limiting and tracking
    pub(crate) fps_limit: Option<f64>, // None for unlimited, Some(fps) for limited
    pub(crate) last_frame_time: std::time::Instant,
    frame_count: u64,
    pub(crate) fps_display: f64,
    fps_update_timer: std::time::Instant,
    pub(crate) return_to_kernel_setup: bool,
    pub(crate) show_settings: bool,
    // Small-body (SBDB) manager UI state
    pub(crate) show_sb_manager: bool,
    pub(crate) sb_download_id_input: String,
    pub(crate) sb_status: Option<String>,
    pub(crate) sb_selected_id: Option<i32>,
    pub(crate) sb_pending_delete: Option<Vec<i32>>,
    pub(crate) sb_download_in_progress: bool,
    pub(crate) sb_download_result_rx: Option<mpsc::Receiver<Result<(), String>>>,
    pub(crate) sb_current_download_id: Option<i32>,
    pub(crate) sbdb_naif_distance_target_id: i32,
    pub(crate) sbdb_naif_search: String,
    pub(crate) show_sbdb_naif_search: bool,

    // Integrator UI state
    pub(crate) show_integrator_window: bool,
    pub(crate) integrator_sb_id: Option<i32>,
    pub(crate) integrator_start_year: i32,
    pub(crate) integrator_start_month: u32,
    pub(crate) integrator_start_day: u32,
    pub(crate) integrator_start_hour: u32,
    pub(crate) integrator_start_minute: u32,
    pub(crate) integrator_start_second: u32,
    pub(crate) integrator_end_year: i32,
    pub(crate) integrator_end_month: u32,
    pub(crate) integrator_end_day: u32,
    pub(crate) integrator_end_hour: u32,
    pub(crate) integrator_end_minute: u32,
    pub(crate) integrator_end_second: u32,
    pub(crate) integrator_dt_hours: f64,
    pub(crate) integrator_in_progress: bool,
    pub(crate) integrator_result_rx: Option<mpsc::Receiver<crate::modules::projection_3d::traj::Integrator>>,
    pub(crate) integrator_progress_rx: Option<mpsc::Receiver<f32>>,
    pub(crate) integrator_progress: f32,
    pub(crate) integrator_proximity_reference: ProximityReference,
    pub(crate) integrator_proximity_multiplier: u32,
    pub(crate) show_closest_approach_markers: bool,
    pub(crate) closest_approach_show_surface_distance: std::collections::HashSet<usize>,
    pub(crate) now_button_highlighted: bool,

    pub(crate) settings_loaded: bool,
    pub(crate) settings_dirty: bool,
    /// for kernel settings UI
    pub(crate) kernel_group_selection: Vec<GroupInfoShort>,
    pub(crate) kernel_reload_error: Option<String>,
    pub(crate) kernel_reload_pending: bool,
    /// populated from settings.toml
    pub(crate) groups_to_load: Vec<String>,

    pub(crate) show_advanced_kernel_options: bool,
    /// group_id -> Vec of selected file indices
    pub(crate) advanced_kernel_selections: std::collections::HashMap<String, Vec<usize>>,
}

impl AppState {
    pub async fn new(
        window: Arc<Window>,
        kernel_selection: KernelStartupSelection,
    ) -> anyhow::Result<AppState> {
        let size = window.inner_size();

        // The instance is a handle to our GPU
        // BackendBit::PRIMARY => Vulkan + Metal + DX12 + Browser WebGPU
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,

                    required_features: adapter.features() & wgpu::Features::all_webgpu_mask(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),

                    required_limits: wgpu::Limits::defaults(),
                    memory_hints: Default::default(),
                    trace: wgpu::Trace::Off, // Trace path
                },
            )
            .await
            .unwrap();

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    // diffuse texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // normal texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("texture_bind_group_layout"),
        });

        let depth_texture = texture::Texture::create_depth_texture(&device, &config, "depth_texture");

        let hdr = HdrPipeline::new(&device, &config);

        let camera = camera::Camera::new((0.0, 500000000.0, 10.0), cgmath::Deg(00.0), cgmath::Deg(-30.0));
        let projection = camera::Projection::new(config.width, config.height, cgmath::Deg(45.0), 0.001);
        let camera_controller = camera::CameraController::new(4.0, 0.4);

        let mut camera_uniform = CameraUniform::new();
        camera_uniform.update_view_proj(&camera, &projection);
        
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniforms = Uniforms::new([config.width as f32, config.height as f32]);
        let uniforms_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniforms Buffer"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }
            ],
            label: Some("camera_bind_group_layout"),
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniforms_buffer.as_entire_binding(),
                }
            ],
            label: Some("camera_bind_group"),
        });

        let mut simulation = Simulation::new_with_kernel_selection(
            CoordSystem::BodyCentric,
            &kernel_selection.groups,
            &kernel_selection.files,
        ).map_err(anyhow::Error::msg)?;
        simulation.initialize_instances();
        let initial_show_barycenters = simulation.show_barycenters;
        let render_origin = simulation.render_origin();
        let cached_sim_time_et = simulation.current_time_et();
        let sun_light_uniform = simulation.sun_light_uniform(render_origin);

        let sun_light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SunLight VB"),
            contents: bytemuck::cast_slice(&[sun_light_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let sun_light_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: None,
        });

        let sun_light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &sun_light_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sun_light_buffer.as_entire_binding(),
            }],
            label: None,
        });

        let environment_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("environment_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::Cube,
                            multisampled: false,
                        },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
 
        let sun_light_render_pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("SunLight Pipeline Layout"),
                bind_group_layouts: &[&camera_bind_group_layout, &sun_light_bind_group_layout],
                push_constant_ranges: &[],
            });
            let shader = wgpu::ShaderModuleDescriptor {
                label: Some("Light Shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/light.wgsl").into()),
            };
            create_render_pipeline_default(
                &device,
                &layout,
                wgpu::TextureFormat::Rgba16Float,
                Some(texture::Texture::DEPTH_FORMAT),
                &[model::ModelVertex::desc()],
                wgpu::PrimitiveTopology::TriangleList,
                shader,
                true,
                wgpu::CompareFunction::GreaterEqual,
            )
        };

        let orbit_pipeline = orbit::OrbitPipeline::new(
            &device,
            &queue,
            &texture_bind_group_layout,
            &camera_bind_group_layout,
            wgpu::TextureFormat::Rgba16Float,
            Some(texture::Texture::DEPTH_FORMAT),
            1024,
            [255, 255, 255, 200],
            &simulation.orbit_instances,
        );

        let trajectory_pipeline = trajectory::TrajectoryPipeline::new(
            &device,
            &camera_bind_group_layout,
            wgpu::TextureFormat::Rgba16Float,
            Some(texture::Texture::DEPTH_FORMAT),
            &Vec::<[f32;3]>::new(),
            &Vec::<u32>::new(),
        );

        let celestial_marker_pipeline = celestial_marker::CelestialMarkerPipeline::new(
            &device,
            &queue,
            &texture_bind_group_layout,
            &camera_bind_group_layout,
            wgpu::TextureFormat::Rgba16Float,
            Some(texture::Texture::DEPTH_FORMAT),
            [255, 255, 0, 255], 
            &simulation.celestial_marker_instances,
            wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
            simulation.visible_ids()
        );

        let closest_approach_pipeline = closest_approach::ClosestApproachPipeline::new(
            &device,
            &queue,
            &texture_bind_group_layout,
            &camera_bind_group_layout,
            wgpu::TextureFormat::Rgba16Float,
            Some(texture::Texture::DEPTH_FORMAT),
            [255, 0, 0, 255],
            &simulation.closest_approach_instances,
            wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
            simulation.closest_approach_visible_ids(),
        );

        let sphere_model = Arc::new(
        resources::load_model("sphere.obj", &device, &queue, &texture_bind_group_layout)
            .await
            .unwrap()
        );

        let body_pipeline = body::BodyPipeline::new(
            &device,
            &camera_bind_group_layout,
            &sun_light_bind_group_layout,
            wgpu::TextureFormat::Rgba16Float,
            Some(texture::Texture::DEPTH_FORMAT),
            sphere_model.clone(),
            &simulation.body_instances,
        );

        let environment_bind_group: wgpu::BindGroup;
        let sky_pipeline: wgpu::RenderPipeline;

        // let hdr_loader = resources::HdrLoader::new(&device);
        // let sky_bytes = resources::load_binary("env_map/starmap_4k.hdr").await?;
        // let sky_texture = hdr_loader.from_equirectangular_bytes(
        //     &device,
        //     &queue,
        //     &sky_bytes,
        //     4096,
        //     Some("Sky Texture"),
        // )?;

        // Create a small dummy cubemap for dark background
        let sky_texture = texture::CubeTexture::create_2d(
            &device,
            1,
            1,
            wgpu::TextureFormat::Rgba32Float,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            wgpu::FilterMode::Nearest,
            Some("Dummy Sky Texture"),
        );

        // Fill with black color
        let black_data = [0.0f32, 0.0, 0.0, 1.0];
        for face in 0..6 {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: sky_texture.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: face },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&black_data),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(16),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
        }

        environment_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("environment_bind_group"),
            layout: &environment_layout,
            entries: &[
                // wgpu::BindGroupEntry {
                //     binding: 0,
                //     resource: wgpu::BindingResource::TextureView(&sky_texture.view()),
                // },
                // wgpu::BindGroupEntry {
                //     binding: 1,
                //     resource: wgpu::BindingResource::Sampler(sky_texture.sampler()),
                // },
                // Dummy entries to satisfy layout (not used in shader)
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sky_texture.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sky_texture.sampler()),
                },
            ],
        });

        sky_pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Sky Pipeline Layout"),
                bind_group_layouts: &[&camera_bind_group_layout, &environment_layout],
                push_constant_ranges: &[],
            });
            let shader = wgpu::include_wgsl!("../shaders/sky.wgsl");
            create_render_pipeline_default(
                &device,
                &layout,
                hdr.format(),
                Some(texture::Texture::DEPTH_FORMAT),
                &[],
                wgpu::PrimitiveTopology::TriangleList,
                shader,
                false,
                wgpu::CompareFunction::GreaterEqual,
            )
        };

        let egui_renderer = EguiRenderer::new(
            &device,
            config.format,
            None,
            1,
            &window,
        );

        // Initialize integrator start/end times from current simulation UTC time (always "YYYY-MM-DD HH:MM:SS.mmm").
        let cur_utc = simulation.current_time_utc().to_string();
        let truncated = &cur_utc[..std::cmp::min(cur_utc.len(), 19)];
        let parsed_dt = NaiveDateTime::parse_from_str(truncated, "%Y-%m-%d %H:%M:%S")
            .unwrap_or_else(|_| NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            ));
        let start_year = parsed_dt.year();
        let start_month = parsed_dt.month();
        let start_day = parsed_dt.day();
        let start_hour = parsed_dt.hour();
        let start_minute = parsed_dt.minute();
        let start_second = parsed_dt.second();

        // Default end time: ~5 months after start (approx. 150 days).
        let end_dt = parsed_dt + Duration::days(150);
        let end_year = end_dt.year();
        let end_month = end_dt.month();
        let end_day = end_dt.day();
        let end_hour = end_dt.hour();
        let end_minute = end_dt.minute();
        let end_second = end_dt.second();

        let state = Self {
            surface,
            device,
            queue,
            config,
            is_surface_configured: false,

            camera,
            camera_uniform,
            projection: projection,
            camera_buffer,
            uniforms,
            uniforms_buffer,
            camera_bind_group,
            camera_controller,

            depth_texture: depth_texture,

            orbit_pipeline,

            celestial_marker_pipeline,
            closest_approach_pipeline,
            body_pipeline,
            trajectory_pipeline,
            hdr,
            environment_bind_group,
            sky_pipeline,

            sun_light_uniform: sun_light_uniform,
            sun_light_buffer: sun_light_buffer,
            sun_light_bind_group: sun_light_bind_group,
            sun_light_render_pipeline: sun_light_render_pipeline,

            sphere_model: sphere_model,
            simulation,
            render_origin,
            
            window,
            right_pressed: false,
            mouse_x: 0.0,
            mouse_y: 0.0,
            need_celestial_picking: false,

            cached_focused_id: None,
            cached_target_pos: cgmath::Point3::new(0.0, 0.0, 0.0),
            cached_min_distance: 1.0,
            cached_show_barycenters: initial_show_barycenters,
            cached_sim_time_et: cached_sim_time_et,
            
            set_year: 2024,
            set_month: 1,
            set_day: 1,
            set_hour: 0,
            set_minute: 0,
            set_second: 0,
            time_error: None,
            egui_renderer,

            // FPS limiting and tracking
            fps_limit: Some(60.0), // Default to 60 FPS
            last_frame_time: std::time::Instant::now(),
            frame_count: 0,
            fps_display: 0.0,
            fps_update_timer: std::time::Instant::now(),
            return_to_kernel_setup: false,
            show_settings: false,
            show_sb_manager: false,
            sb_download_id_input: String::new(),
            sb_status: None,
            sb_selected_id: None,
            sb_pending_delete: None,
            sb_download_in_progress: false,
            sb_download_result_rx: None,
            sb_current_download_id: None,
            sbdb_naif_distance_target_id: 399,
            sbdb_naif_search: String::new(),
            show_sbdb_naif_search: false,

            show_integrator_window: false,
            integrator_start_year: start_year,
            integrator_start_month: start_month,
            integrator_start_day: start_day,
            integrator_start_hour: start_hour,
            integrator_start_minute: start_minute,
            integrator_start_second: start_second,
            integrator_end_year: end_year,
            integrator_end_month: end_month,
            integrator_end_day: end_day,
            integrator_end_hour: end_hour,
            integrator_end_minute: end_minute,
            integrator_end_second: end_second,
            integrator_sb_id: None,
            integrator_dt_hours: 2.0,
            integrator_in_progress: false,
            integrator_result_rx: None,
            integrator_progress_rx: None,
            integrator_progress: 0.0,
            integrator_proximity_reference: ProximityReference::HillSphere,
            integrator_proximity_multiplier: 1,
            show_closest_approach_markers: true,
            closest_approach_show_surface_distance: std::collections::HashSet::new(),
            now_button_highlighted: true,
            settings_loaded: false,
            settings_dirty: false,
            kernel_group_selection: spice_ker::get_all_group_infos_short()
                .unwrap_or_default()
                .into_iter()
                .map(|mut info| {
                    info.is_loaded = kernel_selection.groups.contains(&info.id);
                    info
                })
                .collect(),
            kernel_reload_error: None,
            kernel_reload_pending: false,
            groups_to_load: kernel_selection.groups,
            show_advanced_kernel_options: false,
            advanced_kernel_selections: kernel_selection.files,
            cached_trajectory_flyby: None,
        };

        Ok(state)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.projection.resize(width, height);
            self.hdr.resize(&self.device, width, height);

            self.uniforms = Uniforms::new([width as f32, height as f32]);
            self.queue.write_buffer(
                &self.uniforms_buffer,
                0,
                bytemuck::cast_slice(&[self.uniforms]),
            );

            self.is_surface_configured = true;
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            self.depth_texture = texture::Texture::create_depth_texture(&self.device, &self.config, "depth_texture");

            self.celestial_marker_pipeline.resize(&self.device, width, height);
            self.closest_approach_pipeline.resize(&self.device, width, height);
        }
    }

    pub fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: KeyCode, pressed: bool) {
        if !self.camera_controller.handle_key(key, pressed) {
            match (key, pressed) {
                (KeyCode::Escape, true) => {
                    utils::clear_spice_m();
                    event_loop.exit()
                },
                // Time warp via keyboard: use comma ('<') to step down,
                // and period ('>') to step up through predefined speeds.
                (KeyCode::Comma, true) => {
                    let speeds: [f64; 7] = [
                        1.0,
                        10.0,
                        100.0,
                        1000.0,
                        10000.0,
                        100000.0,
                        1000000.0,
                    ];
                    let current = self.simulation.time_speed();
                    let mut closest_idx: usize = 0;
                    let mut closest_diff = f64::MAX;
                    for (i, v) in speeds.iter().enumerate() {
                        let diff = (current - *v).abs();
                        if diff < closest_diff {
                            closest_diff = diff;
                            closest_idx = i;
                        }
                    }
                    if closest_idx > 0 {
                        self.simulation.set_time_speed(speeds[closest_idx - 1]);
                        self.now_button_highlighted = false;
                        self.settings_dirty = true;
                    }
                }
                (KeyCode::Period, true) => {
                    let speeds: [f64; 7] = [
                        1.0,
                        10.0,
                        100.0,
                        1000.0,
                        10000.0,
                        100000.0,
                        1000000.0,
                    ];
                    let current = self.simulation.time_speed();
                    let mut closest_idx: usize = 0;
                    let mut closest_diff = f64::MAX;
                    for (i, v) in speeds.iter().enumerate() {
                        let diff = (current - *v).abs();
                        if diff < closest_diff {
                            closest_diff = diff;
                            closest_idx = i;
                        }
                    }
                    if closest_idx + 1 < speeds.len() {
                        self.simulation.set_time_speed(speeds[closest_idx + 1]);
                        self.now_button_highlighted = false;
                        self.settings_dirty = true;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        match button {
            MouseButton::Right => self.right_pressed = pressed,
            MouseButton::Left => {
                if pressed {
                    self.need_celestial_picking = true;
                }
            }
            _ => {}
        }
    }

    pub fn handle_mouse_scroll(&mut self, delta: &MouseScrollDelta) {
        self.camera_controller.handle_scroll(delta);
    }

    fn calculate_camera_target(&self) -> (cgmath::Point3<f32>, f32) {
        let target_id = self.simulation.focused_body_id;

        // Determine min distance and position depending on focused body type
        let (target_pos, min_distance) = match self.simulation.focused_body_type {
            crate::modules::projection_3d::simulation::FocusedBodyType::Naif => {
                let min_distance = if let Some(body) = self.simulation.naif_celestial_objects.get(&target_id) {
                    match body.physical_params {
                        Some(p) => ((p.radius() as f32) * 3.0).max(200.0),
                        None => {
                            if target_id == 0 {
                                ((self.simulation.naif_celestial_objects.get(&10).unwrap().physical_params.unwrap().radius() as f32) * 3.0).max(200.0)
                            } else if spice_ker::naif_id_is_barycenter(target_id) {
                                let physical_id = target_id * 100 + 99;
                                if let Some(physical_body) = self.simulation.naif_celestial_objects.get(&physical_id) {
                                    if let Some(p) = physical_body.physical_params {
                                        ((p.radius() as f32) * 3.0).max(200.0)
                                    } else { 1.0 }
                                } else { 1.0 }
                            } else { 1.0 }
                        }
                    }
                } else { 1.0 };

                let target_pos = if let Some(body) = self.simulation.naif_celestial_objects.get(&target_id) {
                    let s = body.glob_state;
                    cgmath::Point3::new(
                        (s.position.x - self.render_origin.x) as f32,
                        (s.position.y - self.render_origin.y) as f32,
                        (s.position.z - self.render_origin.z) as f32,
                    )
                } else { cgmath::Point3::new(0.0, 0.0, 0.0) };
                (target_pos, min_distance)
            }
            crate::modules::projection_3d::simulation::FocusedBodyType::SmallBody => {
                if let Some(sb) = self.simulation.sb_celestial_objects.get(&target_id) {
                    let radius = sb.physical_params
                        .as_ref()
                        .map(|p| p.radius() as f32)
                        .unwrap_or(1.0);
                    let min_distance = (radius * 3.0).max(25000.0);
                    let s = sb.glob_state;
                    let target_pos = cgmath::Point3::new(
                        (s.position.x - self.render_origin.x) as f32,
                        (s.position.y - self.render_origin.y) as f32,
                        (s.position.z - self.render_origin.z) as f32,
                    );
                    (target_pos, min_distance)
                } else {
                    (cgmath::Point3::new(0.0, 0.0, 0.0), 25000.0)
                }
            }
        };

        (target_pos, min_distance)
    }

    pub fn update_gui(&mut self) {
        ui::update_gui(self);
    }

    pub fn take_kernel_setup_request(&mut self) -> bool {
        std::mem::take(&mut self.return_to_kernel_setup)
    }

    pub fn update(&mut self, dt: std::time::Duration) {
        // Always let the simulation update bookkeeping; it will
        // internally skip heavy SPICE work when paused.
        self.simulation.update(dt);

        // Update render origin to follow the focused body
        self.render_origin = self.simulation.render_origin();

        // camera
        let target_id = self.simulation.focused_body_id;
        // Always refresh the target position/min_distance so the camera follows moving targets (Naif or small bodies).
        let (target_pos, min_distance) = self.calculate_camera_target();
        self.cached_target_pos = target_pos;
        self.cached_min_distance = min_distance;
        let focused_changed = self.cached_focused_id != Some(target_id);
        if focused_changed {
            self.cached_focused_id = Some(target_id);
        }
        let barycenters_changed = self.cached_show_barycenters != self.simulation.show_barycenters;
        if barycenters_changed {
            self.cached_show_barycenters = self.simulation.show_barycenters;
        }

        let time_changed = (self.simulation.current_time_et() - self.cached_sim_time_et).abs() > f64::EPSILON;

        self.camera_controller.update_camera_orbital(&mut self.camera, dt, self.cached_target_pos, self.cached_min_distance);
        self.camera_uniform
            .update_view_proj(&self.camera, &self.projection);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );

        // update sun light to use the same origin
        self.sun_light_uniform = self.simulation.sun_light_uniform(self.render_origin);
        self.queue.write_buffer(
            &self.sun_light_buffer,
            0,
            bytemuck::cast_slice(&[self.sun_light_uniform]),
        );

        let need_instance_update = !self.simulation.paused || focused_changed || barycenters_changed || time_changed;

        if need_instance_update {

            self.simulation.upd_ins_pos_ro(self.render_origin);

            self.celestial_marker_pipeline
                .update_instances(&self.device, &self.queue, &self.simulation.celestial_marker_instances, self.simulation.paused, focused_changed);
            self.celestial_marker_pipeline
                .update_visibility(&self.queue, &self.simulation.visible_ids(), &self.simulation.celestial_marker_instances);

            self.orbit_pipeline
                .update_instances(&self.device, &self.queue, &self.simulation.orbit_instances, self.simulation.paused, focused_changed);
            self.orbit_pipeline
                .update_visibility(&self.queue, &self.simulation.orbit_visible_ids(), &self.simulation.orbit_instances);

            self.body_pipeline
                .update_instances(&self.device, &self.queue, &self.simulation.body_instances, self.simulation.paused, focused_changed);
            self.body_pipeline
                .update_visibility(&self.queue, &self.simulation.visible_ids(), &self.simulation.body_instances);

            self.closest_approach_pipeline
                .update_instances(&self.device, &self.queue, &self.simulation.closest_approach_instances, self.simulation.paused, focused_changed);
            self.closest_approach_pipeline
                .update_visibility(&self.queue, &self.simulation.closest_approach_visible_ids(), &self.simulation.closest_approach_instances);

            if let Some((states, flags)) = self.simulation.trajectory_states_with_flags() {
                let origin = self.render_origin;
                let positions_f32: Vec<[f32; 3]> = states
                    .iter()
                    .map(|state_at_epoch| [
                        (state_at_epoch.state.position.x - origin.x) as f32,
                        (state_at_epoch.state.position.y - origin.y) as f32,
                        (state_at_epoch.state.position.z - origin.z) as f32,
                    ])
                    .collect();

                let focused_planet_flyby = self.simulation.integrator.as_ref()
                    .filter(|integrator| integrator.planet_id_in_closest_approaches(self.simulation.focused_body_id))
                    .and_then(|integrator| {
                        let focused_body_id = self.simulation.focused_body_id;
                        if self.cached_trajectory_flyby.as_ref().map(|(id, _)| *id) != Some(focused_body_id) {
                            self.cached_trajectory_flyby = integrator
                                .positions_wrt_planet_flyby(focused_body_id)
                                .map(|segments| (focused_body_id, segments));
                        }
                        self.cached_trajectory_flyby.as_ref().map(|(_, segments)| &segments[..])
                    });

                self.trajectory_pipeline.update_vertices(
                    &self.device,
                    &self.queue,
                    &positions_f32,
                    flags,
                    focused_planet_flyby,
                );
            }
            self.cached_sim_time_et = self.simulation.current_time_et();
        }

        self.update_gui();
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        // FPS limiting logic: if a cap is set, ensure at least the
        // configured frame time has elapsed since the previous frame
        // started before proceeding. This sleep happens before we get
        // the surface texture, so with vsync present modes we don't
        // "double wait".
        if let Some(fps_limit) = self.fps_limit {
            let min_frame_time = std::time::Duration::from_secs_f64(1.0 / fps_limit);
            let elapsed = self.last_frame_time.elapsed();
            if elapsed < min_frame_time {
                std::thread::sleep(min_frame_time - elapsed);
            }
        }

        let now = std::time::Instant::now();
        self.frame_count += 1;
        self.last_frame_time = now;

        // upd FPS display every second
        if self.fps_update_timer.elapsed().as_secs_f64() >= 1.0 {
            self.fps_display = self.frame_count as f64 / self.fps_update_timer.elapsed().as_secs_f64();
            self.frame_count = 0;
            self.fps_update_timer = now;
        }

        if !self.is_surface_configured {
            return Ok(());
        }
        
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.config.format.add_srgb_suffix()),
            ..Default::default()
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        let render_target_view = self.hdr.view();

        // run compute that generates indirect args (visibility -> indirect buffer)
        self.celestial_marker_pipeline.generate_indirect(&mut encoder);
        self.closest_approach_pipeline.generate_indirect(&mut encoder);
        self.orbit_pipeline.generate_indirect(&mut encoder);
        self.body_pipeline.generate_indirect(&mut encoder);

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: render_target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
            });

            render_pass.set_pipeline(&self.sun_light_render_pipeline);
            render_pass.draw_light_model(
                &*self.sphere_model, 
                &self.camera_bind_group, 
                &self.sun_light_bind_group,
            );

            self.orbit_pipeline.draw_orbit_model(&mut render_pass, &self.camera_bind_group);

            self.trajectory_pipeline.draw(&mut render_pass, &self.camera_bind_group);

            self.body_pipeline.draw_body_model(&mut render_pass, &self.camera_bind_group, &self.sun_light_bind_group);

            self.celestial_marker_pipeline.draw_celestial_marker_model(&mut render_pass, &self.camera_bind_group);
            if self.show_closest_approach_markers {
                self.closest_approach_pipeline.draw_celestial_marker_model(&mut render_pass, &self.camera_bind_group);
            }

            render_pass.set_pipeline(&self.sky_pipeline);
            render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
            render_pass.set_bind_group(1, &self.environment_bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }

        self.hdr.process(&mut encoder, &view);

        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: self.window.scale_factor() as f32,
        };
        self.egui_renderer.end_frame_and_draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.window,
            &view,
            screen_descriptor,
        );

        let mut need_celestial_picking_read: bool = false;

        if self.need_celestial_picking {
            self.celestial_marker_pipeline.perform_picking_pass(&mut encoder, &self.camera_bind_group, &self.depth_texture.view);
            self.need_celestial_picking = false;
            need_celestial_picking_read = true;
        }

        // Submit the main rendering (including the picking pass) first
        self.queue.submit(iter::once(encoder.finish()));

        // Then, if a pick was requested, run a separate readback pass that
        // copies the pixel from the picking texture into a staging buffer
        // and maps it on the CPU.
        if need_celestial_picking_read {
            pollster::block_on(self.celestial_marker_pipeline.start_async_picking_readback(
                self.mouse_x,
                self.mouse_y,
                &self.device,
                &self.queue,
                &mut self.simulation,
            ));
        }

        output.present();

        Ok(())
    }

}
