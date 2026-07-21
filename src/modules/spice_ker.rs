use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, LazyLock};

use crate::modules::spice_bindings;
use crate::modules::kernel_config;
use crate::modules::utils;

/// Funcs for testing
pub fn initialize_test_kernels(kernels_path: &Path) -> Result<(), String> {
    let config_path = crate::modules::app_paths::get().kernel_manifest();
    spice_bindings::spice_clear()?;
    furnish_base_kernels(config_path, kernels_path)?;
    initialize_kernel_registry(config_path, kernels_path)?;
    load_kernel_group("inner_solar_system")?;

    Ok(())
}

pub fn get_group_info(group_id: &str) -> Result<(String, (f64, f64), Vec<i32>), String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let group = registry
        .get_group(group_id)
        .ok_or_else(|| format!("Group '{}' not found", group_id))?;

    let time_span = group.time_span();
    let ids = group.active_ids();

    Ok((group.name.clone(), time_span, ids))
}

////////////////////////////////////////////////////////////

fn load_kernels(paths: Vec<PathBuf>) -> Result<(), String> {
    for path in paths {
        let path_str = path.to_string_lossy().to_string();
        spice_bindings::spice_load_kernel(&path_str)
            .map_err(|error| format!("failed to load {}: {}", path.display(), error))?;
    }
    Ok(())
}

/// Loads the base kernels (leap seconds and PCKs) required for basic SPICE operations.
/// Should be called before any other kernel loading or SPICE operations.
pub fn furnish_base_kernels(config_path: &Path, kernels_path: &Path) -> Result<(), String> {
    let config_content = fs::read_to_string(config_path)
        .map_err(|error| format!("Failed to read {}: {}", config_path.display(), error))?;

    let config: kernel_config::KernelConfig = toml::from_str(&config_content)
        .map_err(|error| format!("Failed to parse {}: {}", config_path.display(), error))?;

    let paths = config.base_kernels.kernels.into_iter()
        .map(|kernel| kernels_path.join(kernel.file))
        .collect();

    load_kernels(paths)
}

pub const STANDARD_PLANET_IDS: [i32; 9] = [199, 299, 399, 499, 599, 699, 799, 899, 999];

pub const STANDARD_BARYCENTER_IDS: [i32; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

pub fn naif_id_is_barycenter(id: i32) -> bool {
    id >= 0 && id <= 9
}

pub fn naif_id_is_planet(id: i32) -> bool {
    STANDARD_PLANET_IDS.contains(&id)
}

pub fn naif_id_is_satellite(id: i32) -> bool {
    !naif_id_is_barycenter(id) && id != 10 && !naif_id_is_planet(id)
}

pub struct GroupInfoShort {
    pub id: String,
    pub name: String,
    /// At least one kernel file in the group exists on disk.
    pub is_available: bool,
    pub is_loaded: bool,
}

pub struct FileInfo {
    pub name: String,
    pub time_bounds_utc_start: String,
    pub time_bounds_utc_end: String,
    pub ids_formatted: String,
    pub is_available: bool,
    pub is_selected: bool,
}

#[derive(Debug, Clone)]
pub struct KernelFile {
    pub file_path: String, 
    /// (start_et, end_et) in Ephemeris Time
    pub time_bounds: (f64, f64),
    pub ids: Vec<i32>,
    pub is_loaded: bool,
}

impl KernelFile {
    fn new(
        file_path: String,
        time_bounds: (f64, f64),
        id_ranges: Vec<[i32; 2]>,
    ) -> Self {
        let ids = expand_id_ranges(&id_ranges);
        Self {
            file_path,
            time_bounds,
            ids,
            is_loaded: false,
        }
    }

    pub fn load(&mut self) -> Result<(), String> {
        if !self.is_loaded {
            spice_bindings::spice_load_kernel(&self.file_path)
                .map_err(|error| format!("failed to load {}: {}", self.file_path, error))?;
            self.is_loaded = true;
        }
        Ok(())
    }

    pub fn unload(&mut self) -> Result<(), String> {
        if self.is_loaded {
            spice_bindings::spice_unload_kernel(&self.file_path)
                .map_err(|error| format!("failed to unload {}: {}", self.file_path, error))?;
            self.is_loaded = false;
        }
        Ok(())
    }

    pub fn file_name(&self) -> &str {
        self.file_path.split('/').last().unwrap_or(&self.file_path)
    }

    /// Check if the kernel file exists on disk.
    pub fn is_available(&self) -> bool {
        Path::new(&self.file_path).exists()
    }

    /// (e.g., "3-5, 10, 15-20")
    pub fn ids_formatted(&self) -> String {
        if self.ids.is_empty() {
            return String::new();
        }
        let mut ranges = Vec::new();
        let sorted = {
            let mut s = self.ids.clone();
            s.sort_unstable();
            s.dedup();
            s
        };
        
        let mut i = 0;
        while i < sorted.len() {
            let start = sorted[i];
            let mut end = start;
            while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
                i += 1;
                end = sorted[i];
            }
            if start == end {
                ranges.push(start.to_string());
            } else {
                ranges.push(format!("{}-{}", start, end));
            }
            i += 1;
        }
        ranges.join(", ")
    }
}

#[derive(Debug, Clone)]
pub struct KernelGroup {
    pub id: String,
    pub name: String,
    pub kernels: Vec<KernelFile>,
    pub is_loaded: bool,
    /// For advanced loading. If None, all available files are used.
    pub selected_file_indices: Option<Vec<usize>>,
}

impl KernelGroup {
    pub fn new(
        id: String,
        name: String,
        kernels: Vec<KernelFile>,
    ) -> Self {
        Self {
            id,
            name,
            kernels,
            is_loaded: false,
            selected_file_indices: None,
        }
    }

    /// Those that exist on disk
    pub fn available_files(&self) -> Vec<(usize, &KernelFile)> {
        self.kernels
            .iter()
            .enumerate()
            .filter(|(_, k)| k.is_available())
            .collect()
    }

    /// Check if has some selected files unchecked
    pub fn is_incomplete(&self) -> bool {
        let available = self.available_files();
        if available.is_empty() {
            return false; // group not available at all, not incomplete
        }
        if let Some(selected) = &self.selected_file_indices {
            selected.len() < available.len()
        } else {
            false // all available files selected by default
        }
    }

    /// Selected or all available if not configured
    pub fn active_files(&self) -> Vec<&KernelFile> {
        if let Some(indices) = &self.selected_file_indices {
            indices
                .iter()
                .filter_map(|&i| self.kernels.get(i))
                .filter(|kernel| kernel.is_available())
                .collect()
        } else {
            self.available_files().iter().map(|(_, k)| *k).collect()
        }
    }

    /// Intersection time bounds of active kernel files
    pub fn time_span(&self) -> (f64, f64) {
        let active = self.active_files();
        if active.is_empty() {
            return (0.0, 0.0);
        }

        let mut start = f64::NEG_INFINITY;
        let mut end = f64::INFINITY;

        for kernel in active {
            start = start.max(kernel.time_bounds.0);
            end = end.min(kernel.time_bounds.1);
        }

        (start, end)
    }

    /// Returns a sorted list of unique NAIF IDs from the active kernel files.
    pub fn active_ids(&self) -> Vec<i32> {
        let mut ids = Vec::new();
        for kernel in self.active_files() {
            ids.extend(&kernel.ids);
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn load(&mut self) -> Result<(), String> {
        for (idx, kernel) in self.kernels.iter_mut().enumerate() {
            let should_load = if let Some(selected) = &self.selected_file_indices {
                selected.contains(&idx) && kernel.is_available()
            } else {
                kernel.is_available()
            };
            if should_load {
                if let Err(error) = kernel.load() {
                    for loaded_kernel in &mut self.kernels {
                        let _ = loaded_kernel.unload();
                    }
                    return Err(error);
                }
            }
        }
        self.is_loaded = true;
        Ok(())
    }

    pub fn unload(&mut self) -> Result<(), String> {
        let mut first_error = None;
        for kernel in &mut self.kernels {
            if let Err(error) = kernel.unload() {
                first_error.get_or_insert(error);
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        self.is_loaded = false;
        Ok(())
    }

    pub fn is_available(&self) -> bool {
        self.kernels.iter().any(|k| k.is_available())
    }
}


pub struct KernelRegistry {
    pub groups: HashMap<String, KernelGroup>,
    pub loaded_groups: Vec<String>,
}

impl KernelRegistry {
    fn new() -> Self {
        Self {
            groups: HashMap::new(),
            loaded_groups: Vec::new(),
        }
    }

    pub fn get_group(&self, id: &str) -> Option<&KernelGroup> {
        self.groups.get(id)
    }

    pub fn get_group_mut(&mut self, id: &str) -> Option<&mut KernelGroup> {
        self.groups.get_mut(id)
    }

    pub fn load_group(&mut self, id: &str) -> Result<(), String> {
        let group = self.groups.get_mut(id)
            .ok_or_else(|| format!("kernel group '{}' does not exist", id))?;
        group.load()?;
        if !self.loaded_groups.contains(&id.to_string()) {
            self.loaded_groups.push(id.to_string());
        }
        Ok(())
    }

    pub fn unload_group(&mut self, id: &str) -> Result<(), String> {
        let group = self.groups.get_mut(id)
            .ok_or_else(|| format!("kernel group '{}' does not exist", id))?;
        group.unload()?;
        self.loaded_groups.retain(|g| g != id);
        Ok(())
    }

    /// Returns a list of group IDs that are available (i.e., have at least one kernel file present on disk).
    pub fn available_group_ids(&self) -> Vec<String> {
        self.groups
            .iter()
            .filter(|(_, g)| g.is_available())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn current_time_bounds(&self) -> (f64, f64) {
        let mut start = f64::NEG_INFINITY;
        let mut end = f64::INFINITY;

        for id in &self.loaded_groups {
            if let Some(group) = self.groups.get(id) {
                let (s, e) = group.time_span();
                start = start.max(s);
                end = end.min(e);
            }
        }

        if start.is_infinite() || end.is_infinite() {
            (0.0, 0.0)
        } else {
            (start, end)
        }
    }
}

pub static KERNEL_REGISTRY: LazyLock<Mutex<KernelRegistry>> = LazyLock::new(|| Mutex::new(KernelRegistry::new()));

pub fn initialize_kernel_registry(config_path: &Path, kernels_path: &Path) -> Result<(), String> {
    let config_content = fs::read_to_string(config_path)
        .map_err(|error| format!("Failed to read {}: {}", config_path.display(), error))?;

    let config: kernel_config::KernelConfig = toml::from_str(&config_content)
        .map_err(|error| format!("Failed to parse {}: {}", config_path.display(), error))?;

    let mut registry = KERNEL_REGISTRY.lock().unwrap();
    *registry = KernelRegistry::new();

    for (group_id, group_config) in config.groups {
        let mut kernel_files = Vec::new();

        for kernel_config in group_config.kernels {
            let kernel_path = kernels_path.join(kernel_config.file);
            
            // Convert UTC strings from TOML to ET for storage
            let start_et = utils::utc_to_et(&kernel_config.time_bounds[0])
                .map_err(|e| format!(
                    "Failed to convert start time '{}' to ET: {}",
                    kernel_config.time_bounds[0], e
                ))?;
            let end_et = utils::utc_to_et(&kernel_config.time_bounds[1])
                .map_err(|e| format!(
                    "Failed to convert end time '{}' to ET: {}",
                    kernel_config.time_bounds[1], e
                ))?;
            
            let kernel_file = KernelFile::new(
                kernel_path.to_string_lossy().into_owned(),
                (start_et, end_et),
                kernel_config.ids,
            );
            kernel_files.push(kernel_file);
        }

        let group = KernelGroup::new(
            group_id.clone(),
            group_config.name,
            kernel_files,
        );

        registry.groups.insert(group_id, group);
    }

    Ok(())
}

/// For example, [[1, 3], [5, 5]] becomes [1, 2, 3, 5].
pub fn expand_id_ranges(ranges: &[[i32; 2]]) -> Vec<i32> {
    let mut ids = Vec::new();
    for [start, end] in ranges {
        for id in *start..=*end {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

pub fn load_kernel_group(group_id: &str) -> Result<(), String> {
    let mut registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;
    registry.load_group(group_id)
}

pub fn unload_kernel_group(group_id: &str) -> Result<(), String> {
    let mut registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;
    registry.unload_group(group_id)
}

pub fn get_current_time_bounds() -> Result<(f64, f64), String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;
    Ok(registry.current_time_bounds())
}

pub fn get_available_groups() -> Result<Vec<String>, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;
    Ok(registry.available_group_ids())
}

pub fn is_group_loaded(group_id: &str) -> Result<bool, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    Ok(registry
        .get_group(group_id)
        .map(|g| g.is_loaded)
        .unwrap_or(false))
}

pub fn list_loaded_groups() -> Result<Vec<String>, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    Ok(registry.loaded_groups.clone())
}

pub fn get_all_group_infos_short() -> Result<Vec<GroupInfoShort>, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let mut infos: Vec<GroupInfoShort> = registry
        .groups
        .values()
        .map(|g| GroupInfoShort {
            id: g.id.clone(),
            name: g.name.clone(),
            is_available: g.is_available(),
            is_loaded: g.is_loaded,
        })
        .collect();
    infos.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(infos)
}

pub fn get_group_files_advanced(group_id: &str) -> Result<Vec<FileInfo>, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let group = registry
        .get_group(group_id)
        .ok_or_else(|| format!("Group '{}' not found", group_id))?;

    let result: Vec<FileInfo> = group
        .kernels
        .iter()
        .enumerate()
        .map(|(idx, kernel)| {
            let is_available = kernel.is_available();
            let is_selected = if let Some(selected) = &group.selected_file_indices {
                selected.contains(&idx) && is_available
            } else {
                is_available
            };
            let start_utc = utils::et_to_utc(kernel.time_bounds.0).unwrap_or_default();
            let end_utc = utils::et_to_utc(kernel.time_bounds.1).unwrap_or_default();
            FileInfo {
                name: kernel.file_name().to_string(),
                time_bounds_utc_start: start_utc,
                time_bounds_utc_end: end_utc,
                ids_formatted: kernel.ids_formatted(),
                is_available,
                is_selected,
            }
        })
        .collect();

    Ok(result)
}

/// Changes selected files for a group. Pass indices of files to load. Empty vec = all available.
pub fn set_group_file_selection(group_id: &str, selected_indices: Vec<usize>) -> Result<(), String> {
    let mut registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let group = registry
        .get_group_mut(group_id)
        .ok_or_else(|| format!("Group '{}' not found", group_id))?;

    if selected_indices.is_empty() {
        group.selected_file_indices = None;
    } else {
        group.selected_file_indices = Some(selected_indices);
    }
    Ok(())
}

/// If incomplete, some available files are not selected
pub fn is_group_incomplete(group_id: &str) -> Result<bool, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let group = registry
        .get_group(group_id)
        .ok_or_else(|| format!("Group '{}' not found", group_id))?;

    Ok(group.is_incomplete())
}

/// from all loaded groups.
pub fn get_all_barycenter_ids() -> Result<Vec<i32>, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let mut ids: Vec<i32> = registry
        .loaded_groups
        .iter()
        .filter_map(|gid| registry.groups.get(gid))
        .flat_map(|g| g.active_ids())
        .filter(|&id| naif_id_is_barycenter(id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// from all loaded groups.
pub fn get_all_planet_ids() -> Result<Vec<i32>, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let mut ids: Vec<i32> = registry
        .loaded_groups
        .iter()
        .filter_map(|gid| registry.groups.get(gid))
        .flat_map(|g| g.active_ids())
        .filter(|&id| naif_id_is_planet(id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// from all loaded groups.
pub fn get_all_satellite_ids() -> Result<Vec<i32>, String> {
    let registry = KERNEL_REGISTRY
        .lock()
        .map_err(|e| format!("Failed to acquire registry lock: {}", e))?;

    let mut ids: Vec<i32> = registry
        .loaded_groups
        .iter()
        .filter_map(|gid| registry.groups.get(gid))
        .flat_map(|g| g.active_ids())
        .filter(|&id| naif_id_is_satellite(id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}
