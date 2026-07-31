use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, mpsc};

use egui_wgpu::ScreenDescriptor;
use serde::Deserialize;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::modules::kernel_manager::{
    self, KernelDirectoryReport, KernelDownload, KernelDownloadGroup, KernelDownloadResume,
};

use super::egui_tools::EguiRenderer;

const MAX_CONCURRENT_DOWNLOADS: usize = 3;
const ERROR_BANNER_FILL: egui::Color32 = egui::Color32::from_rgb(70, 25, 25);
const WARNING_BANNER_FILL: egui::Color32 = egui::Color32::from_rgb(65, 52, 20);
const SETUP_BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.025,
    g: 0.03,
    b: 0.04,
    a: 1.0,
};

/// Kernel groups and files that should be loaded when the simulation starts.
#[derive(Debug, Clone, Default)]
pub struct KernelStartupSelection {
    /// e.g. kernels.toml or custom manifest path
    pub manifest_path: PathBuf,
    /// Active group IDs. Base kernels are loaded separately and are not included.
    pub groups: Vec<String>,
    /// Group ID to selected manifest-file indices.
    pub files: HashMap<String, Vec<usize>>,
}

enum DownloadEvent {
    /// Path, downloaded bytes, and total bytes when the server provides it.
    Progress(PathBuf, u64, Option<u64>),
    /// Path and the final downloaded byte count or error message.
    Finished(PathBuf, Result<u64, String>),
    /// Path and the optional byte count returned by a HEAD request.
    HeadSize(PathBuf, Result<Option<u64>, String>),
    UrlTest(PathBuf, Result<Option<u64>, String>),
}

/// Checking if file really exists on NAIF site
enum UrlTestState {
    Checking,
    Available,
    Failed(String),
}

/// 2 steps just in case
#[derive(Clone, Copy)]
enum DeleteAllConfirmation {
    First,
    Final,
}

#[derive(Default)]
struct DownloadTransferState {
    initial_bytes: u64,
    downloaded: u64,
    total: Option<u64>,
    error: Option<String>,
    started_at: Option<std::time::Instant>,
}

struct KernelSetupFile {
    download: KernelDownload,
    local_state: KernelLocalState,
    size: Option<u64>,
    selected: bool,
    download_state: Option<DownloadTransferState>,
    url_test: Option<UrlTestState>,
}

struct KernelSetupGroup {
    id: String,
    name: String,
    required: bool,
    active: bool,
    files: Vec<KernelSetupFile>,
}

impl KernelSetupGroup {
    fn from_download_group(group: KernelDownloadGroup) -> Self {
        let files: Vec<KernelSetupFile> = group
            .kernels
            .into_iter()
            .map(|download| KernelSetupFile {
                download,
                local_state: KernelLocalState::Missing,
                size: None,
                selected: true,
                download_state: None,
                url_test: None,
            })
            .collect::<Vec<KernelSetupFile>>();

        Self {
            active: group.required || group.id == "inner_solar_system",
            id: group.id,
            name: group.name,
            required: group.required,
            files,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KernelLocalState {
    Ready,
    Interrupted,
    Empty,
    Missing,
}

#[derive(Default, Deserialize)]
struct SavedKernelSelection {
    #[serde(default)]
    loaded_groups: Vec<String>,
    #[serde(default)]
    kernel_file_selections: HashMap<String, Vec<usize>>,
}

pub struct KernelSetupState {
    // Only used before the simulation starts.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,
    egui_renderer: EguiRenderer,

    pub window: Arc<Window>,
    groups: Vec<KernelSetupGroup>,
    integrity_report: Option<KernelDirectoryReport>,
    fatal_error: Option<String>,
    manifest_path: PathBuf,

    // e.g. file 4 waits here while files 1-3 are downloading.
    download_client: Option<reqwest::blocking::Client>,
    pending_downloads: VecDeque<KernelDownload>,
    running_download_paths: Vec<PathBuf>,
    download_event_tx: mpsc::Sender<DownloadEvent>,
    download_event_rx: mpsc::Receiver<DownloadEvent>,

    pending_deletion: Option<KernelDownload>,
    delete_all_confirmation: Option<DeleteAllConfirmation>,
    start_requested: bool,
}

impl KernelSetupState {
    pub async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        Self::new_with_manifest(
            window,
            crate::modules::app_paths::get().kernel_manifest().to_path_buf(),
        )
        .await
    }

    /// Builds the setup screen and loads the kernel data (+ integrity checks).
    pub async fn new_with_manifest(
        window: Arc<Window>,
        manifest_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("kernel setup device"),
                required_features: adapter.features() & wgpu::Features::all_webgpu_mask(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::defaults(),
                memory_hints: Default::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: if capabilities
                .present_modes
                .contains(&wgpu::PresentMode::Fifo)
            {
                wgpu::PresentMode::Fifo
            } else {
                capabilities.present_modes[0]
            },
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        let egui_renderer = EguiRenderer::new(&device, format, None, 1, &window);
        let (download_event_tx, download_event_rx) = mpsc::channel();

        let mut state = Self {
            surface,
            device,
            queue,
            config,
            is_surface_configured: false,
            egui_renderer,
            window,
            groups: Vec::new(),
            integrity_report: None,
            fatal_error: None,
            manifest_path,
            download_client: None,
            pending_downloads: VecDeque::new(),
            running_download_paths: Vec::new(),
            download_event_tx,
            download_event_rx,
            pending_deletion: None,
            delete_all_confirmation: None,
            start_requested: false,
        };
        state.load_catalog();
        state.resize(size.width, size.height);
        Ok(state)
    }

    /// Loads the active manifest, saved choices, and local file states.
    fn load_catalog(&mut self) {
        self.fatal_error = None;
        self.groups.clear();
        self.integrity_report = None;

        let kernels_path = crate::modules::app_paths::get().kernels();
        match kernel_manager::read_kernel_groups(&self.manifest_path, kernels_path) {
            Ok(groups) => {
                self.groups = groups
                    .into_iter()
                    .map(KernelSetupGroup::from_download_group)
                    .collect::<Vec<KernelSetupGroup>>();
                self.restore_saved_selection();
            }
            Err(error) => {
                self.fatal_error = Some(error.to_string());
                return;
            }
        }

        self.scan_integrity();

        match kernel_manager::create_client() {
            Ok(client) => {
                self.download_client = Some(client);
                self.request_missing_sizes();
            }
            Err(error) => self.fatal_error = Some(error.to_string()),
        }
    }

    /// Only reads the kernel fields from the application settings file.
    fn restore_saved_selection(&mut self) {
        let Ok(contents) =
            std::fs::read_to_string(crate::modules::app_paths::get().settings())
        else {
            return;
        };
        let Ok(saved) = toml::from_str::<SavedKernelSelection>(&contents) else {
            return;
        };

        for group in &mut self.groups {
            if saved.loaded_groups.contains(&group.id) {
                group.active = true;
            }
            let Some(indices) = saved.kernel_file_selections.get(&group.id) else {
                continue;
            };
            if indices.is_empty() {
                continue;
            }
            for file in &mut group.files {
                file.selected = false;
            }
            for index in indices {
                if let Some(file) = group.files.get_mut(*index) {
                    file.selected = true;
                }
            }
        }
    }

    /// e.g. after a download, refresh Ready/Missing/Empty/Interrupted for every file.
    fn scan_integrity(&mut self) {
        let kernels_path = crate::modules::app_paths::get().kernels();
        match kernel_manager::verify_kernel_dir_integrity(
            &self.manifest_path,
            kernels_path,
        ) {
            Ok(report) => {
                for group in &mut self.groups {
                    for file in &mut group.files {
                        file.local_state = local_state_from_report(&report, &file.download);
                        file.size = match file.local_state {
                            KernelLocalState::Ready | KernelLocalState::Empty => {
                                filesystem_size(&file.download.destination)
                            }
                            KernelLocalState::Interrupted | KernelLocalState::Missing => None,
                        };
                    }
                }

                self.integrity_report = Some(report);
                self.fatal_error = None;
            }
            Err(error) => self.fatal_error = Some(error.to_string()),
        }
    }

    fn request_missing_sizes(&self) {
        let Some(client) = &self.download_client else {
            return;
        };
        let downloads = self
            .groups
            .iter()
            .flat_map(|group| group.files.iter())
            .filter(|file| {
                file.size.is_none()
                    && matches!(file.local_state, KernelLocalState::Missing | KernelLocalState::Interrupted)
            })
            .map(|file| file.download.clone())
            .collect::<Vec<_>>();

        for download in downloads {
            let client = client.clone();
            let event_tx = self.download_event_tx.clone();
            std::thread::spawn(move || {
                // to not block the ui thread
                let result = kernel_manager::get_kernel_header(&client, &download)
                    .map(|header| header.content_length)
                    .map_err(|error| error.to_string());
                let _ = event_tx.send(DownloadEvent::HeadSize(download.relative_path, result));
            });
        }
    }

    /// Reads progress and finished messages sent by download threads.
    fn process_download_events(&mut self) {
        let mut integrity_changed = false;

        // try_recv does not block the UI. Empty just means no more messages this frame.
        loop {
            let event = match self.download_event_rx.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            };

            match event {
                DownloadEvent::Progress(path, downloaded, total) => {
                    if let Some(file) = self.find_file_by_path_mut(&path) {
                        let download_state = file
                            .download_state
                            .get_or_insert_with(DownloadTransferState::default);
                        if downloaded < download_state.initial_bytes {
                            // The server ignored Range, so resume_download_kernel restarted at zero.
                            download_state.initial_bytes = 0;
                            download_state.started_at = Some(std::time::Instant::now());
                        }
                        download_state.downloaded = downloaded;
                        download_state.total = total;
                    }
                }
                DownloadEvent::Finished(path, result) => {
                    self.running_download_paths
                        .retain(|running_path| running_path != &path);
                    if let Some(file) = self.find_file_by_path_mut(&path) {
                        let download_state = file
                            .download_state
                            .get_or_insert_with(DownloadTransferState::default);
                        match result {
                            Ok(downloaded) => {
                                download_state.downloaded = downloaded;
                                download_state.total = Some(downloaded);
                                download_state.error = None;
                            }
                            Err(error) => download_state.error = Some(error),
                        }
                    }
                    integrity_changed = true;
                }
                DownloadEvent::HeadSize(path, result) => {
                    if let Some(file) = self.find_file_by_path_mut(&path) {
                        file.size = result.ok().flatten();
                    }
                }
                DownloadEvent::UrlTest(path, result) => {
                    if let Some(file) = self.find_file_by_path_mut(&path) {
                        match result {
                            Ok(size) => {
                                file.url_test = Some(UrlTestState::Available);
                                if file.size.is_none() {
                                    file.size = size;
                                }
                            }
                            Err(error) => file.url_test = Some(UrlTestState::Failed(error)),
                        }
                    }
                }
            }
        }

        if integrity_changed {
            self.scan_integrity();
        }
    }

    /// Fills the three download slots from the pending queue.
    /// Either resumes or starts a fresh download for each file.
    /// `thread::spawn` returns immediately, so this loop never waits for a file.
    fn start_pending_downloads(&mut self) {
        while self.running_download_paths.len() < MAX_CONCURRENT_DOWNLOADS {
            let Some(kernel) = self.pending_downloads.pop_front() else {
                break;
            };

            let path = kernel.relative_path.clone();

            // We check this on every kernel, but returns just None if file does not exist/is not .part.
            let resume = match KernelDownloadResume::from_download(&kernel) {
                Ok(resume) => resume,
                Err(error) => {
                    if let Some(file) = self.find_file_by_path_mut(&path) {
                        file.download_state = Some(DownloadTransferState {
                            initial_bytes: 0,
                            downloaded: 0,
                            total: None,
                            error: Some(error.to_string()),
                            started_at: None,
                        });
                    }
                    continue;
                }
            };

            let initial_bytes = resume
                .as_ref()
                .map(KernelDownloadResume::downloaded_bytes)
                .unwrap_or(0);

            self.running_download_paths.push(path.clone());

            if let Some(file) = self.find_file_by_path_mut(&path) {
                file.download_state = Some(DownloadTransferState {
                    initial_bytes,
                    downloaded: initial_bytes,
                    total: None,
                    error: None,
                    started_at: Some(std::time::Instant::now()),
                });
            }

            let worker_client = match &self.download_client {
                Some(client) => client.clone(),
                None => {
                    self.running_download_paths
                        .retain(|running_path| running_path != &path);
                    if let Some(file) = self.find_file_by_path_mut(&path) {
                        file.download_state = Some(DownloadTransferState {
                            initial_bytes,
                            downloaded: initial_bytes,
                            total: None,
                            error: Some("The download client is not available".to_string()),
                            started_at: None,
                        });
                    }
                    continue;
                }
            };
            let worker_event_tx = self.download_event_tx.clone();

            std::thread::spawn(move || {
                let progress_path = path.clone();
                let mut report_progress = |downloaded, total| {
                    let _ = worker_event_tx.send(DownloadEvent::Progress(
                        progress_path.clone(),
                        downloaded,
                        total,
                    ));
                };
                let result = match resume {
                    Some(resume) => kernel_manager::resume_download_kernel(
                        &worker_client,
                        &resume,
                        &mut report_progress,
                    ),
                    None => kernel_manager::download_kernel(
                        &worker_client,
                        &kernel,
                        &mut report_progress,
                    ),
                }
                .map_err(|error| error.to_string());

                let _ = worker_event_tx.send(DownloadEvent::Finished(path, result));
            });
        }
    }

    /// Called by Download/Retry buttons to put one file at the back of the queue.
    fn queue_download(&mut self, kernel: KernelDownload) {
        let path = kernel.relative_path.clone();
        let is_running = self.running_download_paths.contains(&path);
        let is_pending = self
            .pending_downloads
            .iter()
            .any(|queued_download| queued_download.relative_path == path);

        if is_running || is_pending {
            return;
        }

        if self.download_client.is_none() {
            if let Some(file) = self.find_file_by_path_mut(&path) {
                file.download_state = Some(DownloadTransferState {
                    initial_bytes: 0,
                    downloaded: 0,
                    total: None,
                    error: Some("The download client is not available".to_string()),
                    started_at: None,
                });
            }
            return;
        }

        if let Some(file) = self.find_file_by_path_mut(&path) {
            file.download_state = None;
        }

        self.pending_downloads.push_back(kernel);
    }

    /// Queues all missing required files.
    fn queue_required_downloads(&mut self) {
        let kernels: Vec<KernelDownload> = self
            .groups
            .iter()
            .filter(|group| group.required)
            .flat_map(|group| group.files.iter())
            .filter(|file| file.local_state != KernelLocalState::Ready)
            .map(|file| file.download.clone())
            .collect();

        for kernel in kernels {
            self.queue_download(kernel);
        }
    }

    fn test_all_urls(&mut self) {
        let Some(client) = &self.download_client else {
            return;
        };
        for file in self.groups.iter_mut().flat_map(|group| &mut group.files) {
            file.url_test = Some(UrlTestState::Checking);
            let client = client.clone();
            let download = file.download.clone();
            let event_tx = self.download_event_tx.clone();
            std::thread::spawn(move || {
                let result = kernel_manager::get_kernel_header(&client, &download)
                    .map(|header| header.content_length)
                    .map_err(|error| error.to_string());
                let _ = event_tx.send(DownloadEvent::UrlTest(download.relative_path, result));
            });
        }
    }

    /// True when all required files exist and are non-empty.
    fn required_files_ready(&self) -> bool {
        self.integrity_report
            .as_ref()
            .is_some_and(KernelDirectoryReport::is_ready)
    }

    /// True when each required simulation group has something usable selected.
    fn required_selection_is_valid(&self) -> bool {
        self.groups
            .iter()
            .filter(|group| group.required && group.id != "base_kernels")
            .all(|group| {
                group
                    .files
                    .iter()
                    .any(|file| file.selected && file.local_state == KernelLocalState::Ready)
            })
    }

    fn find_file_by_path_mut(&mut self, path: &Path) -> Option<&mut KernelSetupFile> {
        self.groups
            .iter_mut()
            .flat_map(|group| group.files.iter_mut())
            .find(|file| file.download.relative_path == path)
    }

    // Functions below are called directly from app.rs.

    pub fn handle_input(&mut self, event: &WindowEvent) -> egui_winit::EventResponse {
        self.egui_renderer.handle_input(&self.window, event)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        // e.g. minimizing a window can temporarily report a zero-sized surface.
        if width == 0 || height == 0 {
            return;
        }

        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.is_surface_configured = true;
    }

    /// True while files are either waiting in the queue or downloading.
    pub fn downloads_active(&self) -> bool {
        !self.pending_downloads.is_empty() || !self.running_download_paths.is_empty()
    }

    pub fn show_start_error(&mut self, error: String) {
        self.fatal_error = Some(error);
    }

    /// Returns the ready files selected on Start so app.rs can open the simulation.
    pub fn start_sim_selection(&mut self) -> Option<KernelStartupSelection> {
        if !self.start_requested {
            return None;
        }
        self.start_requested = false;

        let mut selection = KernelStartupSelection {
            manifest_path: self.manifest_path.clone(),
            ..Default::default()
        };
        for group in &self.groups {
            if group.id == "base_kernels" || !group.active {
                continue;
            }

            let indices: Vec<usize> = group
                .files
                .iter()
                .enumerate()
                .filter_map(|(index, file)| {
                    (file.selected && file.local_state == KernelLocalState::Ready).then_some(index)
                })
                .collect::<Vec<usize>>();

            if indices.is_empty() {
                continue;
            }

            selection.groups.push(group.id.clone());
            selection.files.insert(group.id.clone(), indices);
        }

        Some(selection)
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        self.process_download_events();
        self.start_pending_downloads();

        if !self.is_surface_configured {
            return Ok(());
        }

        self.egui_renderer.begin_frame(&self.window);
        let ctx = self.egui_renderer.context().clone();
        let mut requested_downloads: Vec<KernelDownload> = Vec::new();
        let mut requested_deletion = None;
        let mut download_required_clicked = false;
        let mut recheck_clicked = false;
        let mut test_urls_clicked = false;
        let mut choose_manifest_clicked = false;
        let mut use_default_manifest_clicked = false;
        let mut delete_all_clicked = false;
        let mut start_clicked = false;
        let kernels_dir = crate::modules::app_paths::get().kernels();
        let running_paths: Vec<PathBuf> = self.running_download_paths.clone();
        let queued_paths: Vec<PathBuf> = self
            .pending_downloads
            .iter()
            .map(|kernel| kernel.relative_path.clone())
            .collect::<Vec<PathBuf>>();
        let required_files_ready = self.required_files_ready();
        let required_selection_is_valid = self.required_selection_is_valid();
        let downloads_active = self.downloads_active();
        let running_count = self.running_download_paths.len();
        let queued_count = self.pending_downloads.len();
        let deletion_pending = self.pending_deletion.is_some();

        // Buttons only record actions here; we apply them after egui releases its borrows.
        egui::CentralPanel::default().show(&ctx, |ui| {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.heading("SPICE Kernel Setup");
                ui.label("Choose the kernel data available to this simulation.");
                ui.label(
                    egui::RichText::new(format!("Kernels directory: {}", kernels_dir.display()))
                        .weak(),
                );
                ui.label(
                    egui::RichText::new(format!("Manifest: {}", self.manifest_path.display()))
                        .weak(),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!downloads_active, egui::Button::new("Choose manifest…"))
                        .clicked()
                    {
                        choose_manifest_clicked = true;
                    }
                    let using_default =
                        self.manifest_path == crate::modules::app_paths::get().kernel_manifest();
                    if ui
                        .add_enabled(
                            !downloads_active && !using_default,
                            egui::Button::new("Use default manifest"),
                        )
                        .clicked()
                    {
                        use_default_manifest_clicked = true;
                    }
                });
            });
            ui.add_space(12.0);

            if let Some(error) = &self.fatal_error {
                egui::Frame::new()
                    .fill(ERROR_BANNER_FILL)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    });
            } else if required_files_ready {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "Required kernels are ready.");
            } else {
                egui::Frame::new()
                    .fill(WARNING_BANNER_FILL)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.heading("Kernel data is required before AXbit can start");
                        ui.label("Download the base and inner-solar-system kernels, then start the simulation.");
                        if ui.button("Download required kernels").clicked() {
                            download_required_clicked = true;
                        }
                    });
            }

            ui.add_space(10.0);
            let catalog_height = (ui.available_height() - 52.0).max(120.0);
            egui::ScrollArea::vertical().max_height(catalog_height).show(ui, |ui| {
                for group in &mut self.groups {
                    let ready_count = group
                        .files
                        .iter()
                        .filter(|file| file.local_state == KernelLocalState::Ready)
                        .count();
                    let required = group.required;
                    let mut active = group.active;

                    egui::CollapsingHeader::new(format!(
                        "{}  ({}/{}){}",
                        group.name,
                        ready_count,
                        group.files.len(),
                        if required { " · required" } else { "" },
                    ))
                    .default_open(required)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if required {
                                ui.add_enabled(false, egui::Checkbox::new(&mut active, "Use in simulation"));
                            } else if ui.checkbox(&mut active, "Use in simulation").changed() {
                                group.active = active;
                            }

                            if ready_count < group.files.len()
                                && ui.small_button("Download missing").clicked()
                            {
                                requested_downloads.extend(
                                    group.files.iter()
                                        .filter(|file| file.local_state != KernelLocalState::Ready)
                                        .map(|file| file.download.clone()),
                                );
                            }
                        });

                        for file in &mut group.files {
                            ui.separator();
                            ui.horizontal(|ui| {
                                let ready = file.local_state == KernelLocalState::Ready;
                                ui.add_enabled(
                                    active && ready,
                                    egui::Checkbox::new(&mut file.selected, ""),
                                );

                                ui.vertical(|ui| {
                                    ui.strong(file.download.relative_path.display().to_string());
                                    show_file_status(
                                        ui,
                                        file,
                                        running_paths.contains(&file.download.relative_path),
                                        queued_paths.contains(&file.download.relative_path),
                                    );
                                    if let Some(size) = file.size {
                                        ui.weak(format!("Size: {}", format_bytes(size)));
                                    }
                                    if let Some(status) = &file.url_test {
                                        match status {
                                            UrlTestState::Checking => {
                                                ui.horizontal(|ui| {
                                                    ui.spinner();
                                                    ui.weak("Testing URL…");
                                                });
                                            }
                                            UrlTestState::Available => {
                                                ui.colored_label(
                                                    egui::Color32::LIGHT_GREEN,
                                                    "URL available",
                                                );
                                            }
                                            UrlTestState::Failed(error) => {
                                                ui.colored_label(
                                                    egui::Color32::LIGHT_RED,
                                                    format!("URL failed: {error}"),
                                                );
                                            }
                                        }
                                    }
                                });

                                let action_label = if file
                                    .download_state
                                    .as_ref()
                                    .is_some_and(|state| state.error.is_some())
                                {
                                    "Retry"
                                } else if file.local_state == KernelLocalState::Interrupted {
                                    "Resume"
                                } else {
                                    "Download"
                                };
                                if !ready
                                    && !running_paths.contains(&file.download.relative_path)
                                    && !queued_paths.contains(&file.download.relative_path)
                                    && ui.small_button(action_label).clicked()
                                {
                                    requested_downloads.push(file.download.clone());
                                }
                                if file.local_state != KernelLocalState::Missing
                                    && !deletion_pending
                                    && !running_paths.contains(&file.download.relative_path)
                                    && !queued_paths.contains(&file.download.relative_path)
                                    && ui.small_button("Delete").clicked()
                                {
                                    requested_deletion = Some(file.download.clone());
                                }
                            });
                        }
                    });
                }
            });

            ui.separator();
            if required_files_ready && !required_selection_is_valid {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Select at least one file from every required simulation group.",
                );
            }
            ui.horizontal(|ui| {
                if ui.button("Recheck").clicked() {
                    recheck_clicked = true;
                }
                if ui.button("Test all URLs").clicked() {
                    test_urls_clicked = true;
                }
                if ui
                    .add_enabled(
                        !downloads_active,
                        egui::Button::new("Delete all kernel files"),
                    )
                    .clicked()
                {
                    delete_all_clicked = true;
                }
                if downloads_active {
                    ui.spinner();
                    ui.label(format!(
                        "{} downloading · {} queued",
                        running_count,
                        queued_count,
                    ));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_start = required_files_ready
                        && required_selection_is_valid
                        && !downloads_active
                        && self.fatal_error.is_none();
                    if ui.add_enabled(can_start, egui::Button::new("Start Simulation")).clicked() {
                        start_clicked = true;
                    }
                });
            });
        });

        if requested_deletion.is_some() {
            self.pending_deletion = requested_deletion.take();
        }
        if delete_all_clicked {
            self.delete_all_confirmation = Some(DeleteAllConfirmation::First);
        }
        let mut confirm_deletion = false;
        let mut cancel_deletion = false;
        if let Some(kernel) = self.pending_deletion.as_ref() {
            let path = kernel.relative_path.display().to_string();
            egui::Window::new("Delete kernel file?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.colored_label(egui::Color32::YELLOW, "Warning: this file will be deleted from disk.");
                    ui.label(path);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            confirm_deletion = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel_deletion = true;
                        }
                    });
                });
        }
        if confirm_deletion {
            requested_deletion = self.pending_deletion.take();
        } else if cancel_deletion {
            self.pending_deletion = None;
        }

        let mut advance_delete_all = false;
        let mut confirm_delete_all = false;
        let mut cancel_delete_all = false;
        match self.delete_all_confirmation {
            Some(DeleteAllConfirmation::First) => {
                egui::Window::new("Delete all kernel files?")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(&ctx, |ui| {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "This deletes every file inside the kernels directory, including files not listed in the manifest.",
                        );
                        ui.label(kernels_dir.display().to_string());
                        ui.horizontal(|ui| {
                            if ui.button("OK, continue").clicked() {
                                advance_delete_all = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel_delete_all = true;
                            }
                        });
                    });
            }
            Some(DeleteAllConfirmation::Final) => {
                egui::Window::new("Final deletion confirmation")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(&ctx, |ui| {
                        ui.colored_label(
                            egui::Color32::RED,
                            egui::RichText::new("ARE YOU SURE?").strong().size(22.0),
                        );
                        ui.label("All downloaded and manually added kernel files will be permanently deleted.");
                        ui.horizontal(|ui| {
                            if ui.button("Yes, delete everything").clicked() {
                                confirm_delete_all = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel_delete_all = true;
                            }
                        });
                    });
            }
            None => {}
        }
        if advance_delete_all {
            self.delete_all_confirmation = Some(DeleteAllConfirmation::Final);
        } else if cancel_delete_all || confirm_delete_all {
            self.delete_all_confirmation = None;
        }

        self.start_requested = start_clicked;

        // e.g. a Download click reaches the pending queue here.
        if download_required_clicked {
            self.queue_required_downloads();
        }
        if test_urls_clicked {
            self.test_all_urls();
        }
        if use_default_manifest_clicked {
            self.manifest_path = crate::modules::app_paths::get()
                .kernel_manifest()
                .to_path_buf();
            self.load_catalog();
        } else if choose_manifest_clicked {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("TOML manifest", &["toml"])
                .pick_file()
            {
                self.manifest_path = path;
                self.load_catalog();
            }
        }
        for kernel in requested_downloads {
            self.queue_download(kernel);
        }
        let mut deletion_error = None;
        if confirm_delete_all {
            if let Err(error) = kernel_manager::delete_kernel_directory_contents(kernels_dir) {
                deletion_error = Some(error.to_string());
            }
            for file in self.groups.iter_mut().flat_map(|group| &mut group.files) {
                file.download_state = None;
                file.url_test = None;
            }
            self.scan_integrity();
            self.request_missing_sizes();
        } else if let Some(kernel) = requested_deletion {
            if let Err(error) = kernel_manager::delete_kernel_files(&kernel) {
                deletion_error = Some(error.to_string());
            }
            if let Some(file) = self.find_file_by_path_mut(&kernel.relative_path) {
                file.download_state = None;
            }
            self.scan_integrity();
            self.request_missing_sizes();
        } else if recheck_clicked {
            self.scan_integrity();
        }
        if let Some(error) = deletion_error {
            self.fatal_error = Some(error);
        }

        // Clear the window, draw egui, then present this startup frame.
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("kernel setup encoder"),
            });
        {
            let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("kernel setup clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(SETUP_BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        self.egui_renderer.end_frame_and_draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.window,
            &view,
            ScreenDescriptor {
                size_in_pixels: [self.config.width, self.config.height],
                pixels_per_point: self.window.scale_factor() as f32,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

/// Shows what the user needs for one file: queued, progress, error, or disk state.
fn show_file_status(ui: &mut egui::Ui, file: &KernelSetupFile, is_running: bool, is_queued: bool) {
    if is_running {
        let Some(download_state) = &file.download_state else {
            ui.label("Starting download...");
            return;
        };
        let fraction = download_state
            .total
            .filter(|total| *total > 0)
            .map(|total| download_state.downloaded as f32 / total as f32);
        let progress_text = format_download_progress(download_state);

        if let Some(fraction) = fraction {
            ui.add(egui::ProgressBar::new(fraction).show_percentage());
            ui.label(progress_text);
        } else {
            ui.label(format!("Downloading · {}", progress_text));
        }
    } else if is_queued {
        ui.label(egui::RichText::new("Queued").color(egui::Color32::LIGHT_BLUE));
    } else if let Some(error) = file
        .download_state
        .as_ref()
        .and_then(|transfer| transfer.error.as_ref())
    {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    } else {
        match file.local_state {
            KernelLocalState::Ready => {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "Ready");
            }
            KernelLocalState::Interrupted => {
                ui.colored_label(egui::Color32::YELLOW, "Interrupted download");
            }
            KernelLocalState::Empty => {
                ui.colored_label(egui::Color32::YELLOW, "Empty or invalid file");
            }
            KernelLocalState::Missing => {
                ui.weak("Not downloaded");
            }
        }
    }
}

fn format_download_progress(download_state: &DownloadTransferState) -> String {
    let elapsed_seconds = download_state
        .started_at
        .map(|started_at| started_at.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    let bytes_per_second = if elapsed_seconds > 0.0 {
        download_state
            .downloaded
            .saturating_sub(download_state.initial_bytes) as f64
            / elapsed_seconds
    } else {
        0.0
    };
    let speed = format!("{}/s", format_bytes(bytes_per_second as u64));

    match download_state.total {
        Some(total) if total > 0 => format!(
            "{} / {} · {}",
            format_bytes(download_state.downloaded),
            format_bytes(total),
            speed,
        ),
        _ => format!("{} · {}", format_bytes(download_state.downloaded), speed),
    }
}

/// Turns the integrity report into the simple state shown next to a file.
fn local_state_from_report(
    report: &KernelDirectoryReport,
    kernel: &KernelDownload,
) -> KernelLocalState {
    let configured_file = report
        .files
        .iter()
        .find(|file| file.relative_path == kernel.relative_path);

    if configured_file.is_some_and(|file| file.is_available) {
        KernelLocalState::Ready
    } else if report
        .incomplete_files
        .contains(&part_path(&kernel.destination))
    {
        KernelLocalState::Interrupted
    } else if configured_file.is_some_and(|file| file.is_empty) {
        KernelLocalState::Empty
    } else {
        KernelLocalState::Missing
    }
}

/// e.g. `de442.bsp` becomes `de442.bsp.part` while it is downloading.
/// Consistent with kernel_downloader.
fn part_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.part", path.display()))
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        return format!("{:.1} MiB", bytes as f64 / MIB);
    } else if bytes >= 1024 {
        return format!("{:.1} KiB", bytes as f64 / 1024.0);
    } else {
        return format!("{} B", bytes);
    }
}

fn filesystem_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
}
