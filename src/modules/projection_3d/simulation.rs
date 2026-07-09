use cgmath::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use log::info;
use crate::modules::{
    spice_ker,
    projection_3d::{
        celestial::{NaifCelestial, SbCelestial}, traj::{ClosestApproach, Integrator, PhysicalParams}, window_core::instance::Instance,
        state::*
    }, 
    utils
};

pub const G: f64 = 6.67430e-11;
pub const AU_KM: f64 = 149597870.7;

fn get_celestial_color(id: i32) -> [f32; 3] {
    match id {
        10 => [1.0, 1.0, 0.9], // Sun: yellow-white
        199 => [0.5, 0.5, 0.5], // Mercury: gray
        299 => [1.0, 0.8, 0.0], // Venus: yellow-orange
        399 => [0.0, 0.5, 1.0], // Earth: blue
        499 => [1.0, 0.3, 0.0], // Mars: red
        599 => [0.8, 0.6, 0.4], // Jupiter: orange-brown
        699 => [0.9, 0.8, 0.6], // Saturn: pale yellow
        799 => [0.4, 0.8, 1.0], // Uranus: cyan
        899 => [0.2, 0.4, 1.0], // Neptune: blue
        _ => [1.0, 1.0, 1.0], // Default white
    }
}

/// Workaround. Cheap enough per frame. 
fn convert_quat_f64_to_f32(q: Quaternion<f64>) -> Quaternion<f32> {
    Quaternion::new(q.s as f32, q.v.x as f32, q.v.y as f32, q.v.z as f32)
}

// /MARK: barocenter-centric is not really implemented yet. 
// DO NOT USE YET.

#[derive(Clone, Copy, Default)]
pub enum CoordSystem {
    #[default]
    BodyCentric = 10, // central body - sun (10)
    BarocenterCentric = 0 // central barycenter - ssb (0)
}

pub struct Simulation {
    current_time_et: f64,
    current_time_utc: String,
    min_time_et: f64,
    max_time_et: f64,
    coord_system: CoordSystem,
    ref_frame: &'static str,
    time_speed: f64,
    upd_kep_elts: bool,

    pub focused_body_id: i32,
    pub focused_body_type: FocusedBodyType,
    pub naif_celestial_objects: Arc<HashMap<i32, NaifCelestial>>,
    pub sb_celestial_objects: HashMap<i32, SbCelestial>,
    pub naif_sorted_ids: Vec<i32>,
    pub sb_sorted_ids: Vec<i32>,
    pub sb_filter: HashSet<i32>,
    pub sb_orbit_filter: HashSet<i32>,
    pub current_visible_ids: Vec<i32>,
    pub available_satellites_ids: Vec<i32>,
    pub planet_ids: Vec<i32>,
    pub barycenter_ids: Vec<i32>,

    pub show_barycenters: bool,
    pub show_small_bodies: bool,
    
    pub orbit_instances: Vec<Instance>,
    pub celestial_marker_instances: Vec<Instance>,
    pub body_instances: Vec<Instance>,
    pub closest_approach_points: Vec<Vec3d>,
    pub closest_approach_instances: Vec<Instance>,

	pub integrator: Option<Integrator>,

    // when true, pipeline update routines should skip writing GPU instance buffers.
    pub paused: bool,
    
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FocusedBodyType {
    Naif,
    SmallBody,
}

impl Simulation {
    pub fn new(coord_system: CoordSystem) -> Result<Self, String> {
        Self::new_with_kernel_selection(
            coord_system,
            &[],
            &std::collections::HashMap::new(),
        )
    }

    pub fn new_with_kernel_selection(
        coord_system: CoordSystem,
        selected_groups: &[String],
        selected_files: &std::collections::HashMap<String, Vec<usize>>,
    ) -> Result<Self, String> {
        let config_path = std::path::Path::new("kernels.toml");
        let kernels_path = std::path::Path::new("spice-tools/kernels");
        // Must come first: LSK and PCK are required for UTC ET conversions
        spice_ker::furnish_base_kernels(config_path, kernels_path)
            .map_err(|e| format!("Failed to load base SPICE kernels: {}", e))?;

        let cur_utc = utils::current_utc().ok_or_else(|| "Could not initialize utc time in Simulation::new()".to_string())?;
        let cur_et = utils::utc_to_et(&cur_utc).map_err(|e| format!("Could not initialize utc time in Simulation::new(): {}", e))?;

        spice_ker::initialize_kernel_registry(config_path, kernels_path)
            .map_err(|e| format!("Failed to initialize kernel registry: {}", e))?;
        for (group_id, indices) in selected_files {
            spice_ker::set_group_file_selection(group_id, indices.clone())
                .map_err(|e| format!("Failed to select files for '{}': {}", group_id, e))?;
        }

        let groups_to_load = if selected_groups.is_empty() {
            spice_ker::get_available_groups()
                .map_err(|e| format!("Failed to get available kernel groups: {}", e))?
        } else {
            selected_groups.to_vec()
        };
        for group_id in &groups_to_load {
            spice_ker::load_kernel_group(group_id)
                .map_err(|e| format!("Failed to load kernel group '{}': {}", group_id, e))?;
        }

        let (min_time_et, max_time_et) = spice_ker::get_current_time_bounds()
            .map_err(|e| format!("Failed to get time bounds from kernel registry: {}", e))?;

        let available_satellites_ids = spice_ker::get_all_satellite_ids()
            .map_err(|e| format!("could not get available satellites ids: {}", e))?;
        let planet_ids = spice_ker::get_all_planet_ids()
            .map_err(|e| format!("could not get planet ids: {}", e))?;
        let barycenter_ids = spice_ker::get_all_barycenter_ids()
            .map_err(|e| format!("could not get barycenter ids: {}", e))?;

        let cur_et = cur_et.clamp(min_time_et, max_time_et);

        let mut v = vec![10];
        let naif_celestial_objects = Arc::new(Self::initialize_naif_celestials(&coord_system, cur_et, "ECLIPJ2000", &available_satellites_ids)?);
        let sb_celestial_objects = Self::initialize_small_bodies(&coord_system, cur_et, "ECLIPJ2000", &naif_celestial_objects);

        let mut naif_sorted_ids: Vec<i32> = naif_celestial_objects.keys().copied().collect();
        naif_sorted_ids.sort_unstable();

        let mut sb_sorted_ids: Vec<i32> = sb_celestial_objects.keys().copied().collect();
        sb_sorted_ids.sort_unstable();

        let sb_filter: HashSet<i32> = sb_celestial_objects.keys().copied().collect();
        let sb_orbit_filter: HashSet<i32> = sb_filter.clone();

        Ok(Simulation { 
            current_time_et: cur_et, 
            current_time_utc: cur_utc,
            min_time_et,
            max_time_et,
            coord_system, 
            ref_frame: "ECLIPJ2000",
            naif_celestial_objects,
            sb_celestial_objects,
            naif_sorted_ids,
            sb_sorted_ids,
            sb_filter,
            sb_orbit_filter,
            focused_body_id: 10,
            focused_body_type: FocusedBodyType::Naif,
            current_visible_ids: match coord_system {
                CoordSystem::BodyCentric => { v.extend_from_slice(&planet_ids); v },
                CoordSystem::BarocenterCentric => { v.extend(barycenter_ids.iter().filter(|&&id| id != 0)); v },
            },
            available_satellites_ids,
            planet_ids,
            barycenter_ids,
            show_barycenters: false,
            show_small_bodies: true,

            orbit_instances: vec![],
            celestial_marker_instances: vec![],
            body_instances: vec![],
            closest_approach_points: vec![],
            closest_approach_instances: vec![],

            integrator: None,

            paused: false,
            time_speed: 1.0,
            upd_kep_elts: false,
        })
    }

    pub fn current_time_et(&self) -> f64 { self.current_time_et }

    /// Unloads all currently loaded groups, loads the new set, then rebuilds NAIF objects.
    pub fn reload_with_groups(&mut self, selected_groups: &[String]) -> Result<(), String> {
        // Unload every currently loaded group
        let loaded = spice_ker::list_loaded_groups()
            .map_err(|e| format!("Failed to list loaded groups: {}", e))?;
        for gid in &loaded {
            spice_ker::unload_kernel_group(gid)
                .map_err(|e| format!("Failed to unload group '{}': {}", gid, e))?;
        }

        // inner_solar_system is always required
        spice_ker::load_kernel_group("inner_solar_system")
            .map_err(|e| format!("Failed to load inner_solar_system: {}", e))?;
        for gid in selected_groups {
            if gid != "inner_solar_system" {
                spice_ker::load_kernel_group(gid)
                    .map_err(|e| format!("Failed to load group '{}': {}", gid, e))?;
            }
        }

        let (min_time_et, max_time_et) = spice_ker::get_current_time_bounds()
            .map_err(|e| format!("Failed to get time bounds: {}", e))?;

        self.available_satellites_ids = spice_ker::get_all_satellite_ids()
            .map_err(|e| format!("Failed to get satellite ids: {}", e))?;
        self.planet_ids = spice_ker::get_all_planet_ids()
            .map_err(|e| format!("Failed to get planet ids: {}", e))?;
        self.barycenter_ids = spice_ker::get_all_barycenter_ids()
            .map_err(|e| format!("Failed to get barycenter ids: {}", e))?;

        self.min_time_et = min_time_et;
        self.max_time_et = max_time_et;
        self.current_time_et = self.current_time_et.clamp(min_time_et, max_time_et);
        self.current_time_utc = utils::et_to_utc(self.current_time_et)
            .unwrap_or_else(|_| "Invalid Time".to_string());

        self.naif_celestial_objects = Arc::new(Self::initialize_naif_celestials(
            &self.coord_system,
            self.current_time_et,
            self.ref_frame,
            &self.available_satellites_ids,
        )?);

        let mut sorted: Vec<i32> = self.naif_celestial_objects.keys().copied().collect();
        sorted.sort_unstable();
        self.naif_sorted_ids = sorted;

        self.focused_body_id = 10;
        self.focused_body_type = FocusedBodyType::Naif;

        let mut v = vec![10];
        self.current_visible_ids = match self.coord_system {
            CoordSystem::BodyCentric => { v.extend_from_slice(&self.planet_ids); v },
            CoordSystem::BarocenterCentric => { v.extend(self.barycenter_ids.iter().filter(|&&id| id != 0).copied()); v },
        };

        Ok(())
    }

    /// (min_et, max_et) intersection time bounds of all loaded kernel groups.
    pub fn time_bounds(&self) -> (f64, f64) { (self.min_time_et, self.max_time_et) }

    /// (start_year, end_year) as integers derived from loaded kernel time bounds.
    pub fn time_bounds_years(&self) -> (i32, i32) {
        let parse_year = |et: f64| -> i32 {
            utils::et_to_utc(et)
                .ok()
                .and_then(|s| s[..4].parse::<i32>().ok())
                .unwrap_or(2000)
        };
        (parse_year(self.min_time_et), parse_year(self.max_time_et))
    }

    /// Used after updating active kernel groups
    pub fn update_time_bounds(&mut self) -> Result<(), String> {
        let (min_time_et, max_time_et) = spice_ker::get_current_time_bounds()
            .map_err(|e| format!("Failed to get updated time bounds: {}", e))?;
        
        self.min_time_et = min_time_et;
        self.max_time_et = max_time_et;
        
        // Clamp current time to new bounds
        if !self.current_time_et.is_finite() || self.current_time_et < min_time_et || self.current_time_et > max_time_et {
            self.current_time_et = self.current_time_et.clamp(min_time_et, max_time_et);
        }
        
        Ok(())
    }

    pub fn coord_system(&self) -> CoordSystem { self.coord_system }

    pub fn time_speed(&self) -> f64 { self.time_speed }

    pub fn set_time_speed(&mut self, new_time_speed: f64) {  
        if self.time_speed > 1.0 && new_time_speed == 1.0 { 
            // if we're going from fast to real-time, 
            // update all orbits to avoid inaccuracies from large time steps
            self.upd_kep_elts = true;
        }
        self.time_speed = new_time_speed;
    }

    fn insert_sorted_unique(ids: &mut Vec<i32>, id: i32) {
        match ids.binary_search(&id) {
            Ok(_) => {}
            Err(pos) => ids.insert(pos, id),
        }
    }

    fn remove_sorted(ids: &mut Vec<i32>, id: i32) {
        if let Ok(pos) = ids.binary_search(&id) {
            ids.remove(pos);
        }
    }

    pub fn set_integrator(&mut self, integrator: Integrator) {
        self.integrator = Some(integrator);
    }

    pub fn clear_integrator(&mut self) {
        self.integrator = None;
        self.clear_closest_approach_points();
    }

    pub fn clear_closest_approach_points(&mut self) {
        self.closest_approach_points.clear();
        self.closest_approach_instances.clear();
    }

    pub fn set_closest_approach_points(&mut self, approaches: &[ClosestApproach], origin: Vec3d) {
        self.closest_approach_points = approaches.iter().map(|approach| approach.position).collect();
        self.closest_approach_instances = approaches
            .iter()
            .map(|approach| {
                let point = approach.position;
                let position = cgmath::Vector3::new(
                    (point[0] - origin[0]) as f32,
                    (point[1] - origin[1]) as f32,
                    (point[2] - origin[2]) as f32,
                );
                let rotation = cgmath::Quaternion::new(1.0, 0.0, 0.0, 0.0);
                let scale = cgmath::Vector3::new(1.0, 1.0, 1.0);
                Instance {
                    position,
                    rotation,
                    scale,
                    color: [1.0, 0.0, 0.0],
                    id: Some(10_000_000 + approach.event_id as i32),
                    is_sbdb: false,
                }
            })
            .collect();
    }

    pub fn closest_approach_visible_ids(&self) -> Vec<i32> {
        self.closest_approach_instances
            .iter()
            .filter_map(|inst| inst.id)
            .collect()
    }

    pub fn trajectory_positions(&self) -> Option<Vec<Vec3d>> {
        self.integrator.as_ref().map(|i| i.positions())
    }

    // Return raw integrator states and proximity flags by reference.
    // This avoids cloning the full trajectory data on every frame when the
    // renderer only needs to convert and upload it once.
    pub fn trajectory_states_with_flags(&self) -> Option<(&[StateAtEpoch], &[u32])> {
        self.integrator
            .as_ref()
            .map(|i| (i.states(), i.proximity_flags_ref()))
    }

    pub fn visible_ids(&self) -> Vec<i32> { 
        let mut res: Vec<i32> = Vec::new();
        res.push(10);

        for celestial in self.naif_celestial_objects.values() {
            if celestial.constituents_ids.contains(&self.focused_body_id) {
                res.append(&mut celestial.constituents_ids.clone());
            }
        }

        match self.coord_system {
            CoordSystem::BodyCentric => {
                res.extend_from_slice(&self.planet_ids);

                if let Some(focused) = self.naif_celestial_objects.get(&self.focused_body_id) {
                    res.append(&mut focused.constituents_ids.clone());
                    if !self.show_barycenters {
                        res.retain(|&id| !self.barycenter_ids.contains(&id));
                    }
                }
                if self.show_barycenters {
                    res.push(0);
                }
                if self.show_small_bodies {
                    for id in self.sb_celestial_objects.keys() {
                        if self.sb_filter.contains(id) {
                            res.push(*id);
                        }
                    }
                }
            },
            CoordSystem::BarocenterCentric => {
                if self.show_barycenters {
                    res.extend(self.barycenter_ids.iter().copied());
                }

                if let Some(focused) = self.naif_celestial_objects.get(&self.focused_body_id) {
                    res.append(&mut focused.constituents_ids.clone());
                }
                if self.show_small_bodies {
                    for id in self.sb_celestial_objects.keys() {
                        if self.sb_filter.contains(id) {
                            res.push(*id);
                        }
                    }
                }
            },
        }
        res
    }

    pub fn orbit_visible_ids(&self) -> Vec<i32> { 
        let mut res: Vec<i32> = Vec::new();
        res.push(10);

        for celestial in self.naif_celestial_objects.values() {
            if celestial.constituents_ids.contains(&self.focused_body_id) {
                res.append(&mut celestial.constituents_ids.clone());
            }
        }

        match self.coord_system {
            CoordSystem::BodyCentric => {
                res.extend_from_slice(&self.planet_ids);

                if let Some(focused) = self.naif_celestial_objects.get(&self.focused_body_id) {
                    res.append(&mut focused.constituents_ids.clone());
                    if !self.show_barycenters {
                        res.retain(|&id| !self.barycenter_ids.contains(&id));
                    }
                }
                if self.show_barycenters {
                    res.push(0);
                }
                if self.show_small_bodies {
                    for id in self.sb_celestial_objects.keys() {
                        if self.sb_filter.contains(id) && self.sb_orbit_filter.contains(id) {
                            res.push(*id);
                        }
                    }
                }
            },
            CoordSystem::BarocenterCentric => {
                if self.show_barycenters {
                    res.extend(self.barycenter_ids.iter().copied());
                }

                if let Some(focused) = self.naif_celestial_objects.get(&self.focused_body_id) {
                    res.append(&mut focused.constituents_ids.clone());
                }
                if self.show_small_bodies {
                    for id in self.sb_celestial_objects.keys() {
                        if self.sb_filter.contains(id) && self.sb_orbit_filter.contains(id) {
                            res.push(*id);
                        }
                    }
                }
            },
        }
        res
    }

    pub fn render_origin(&self) -> Vec3d {
        if let Some(body) = self.naif_celestial_objects.get(&self.focused_body_id) {
            body.glob_state.position
        } else {
            Vec3d::new(0.0, 0.0, 0.0)
        }
    }
    
    fn initialize_naif_celestials(coord_system: &CoordSystem, time_et: f64, ref_frame: &'static str, available_satellites_ids: &Vec<i32>) -> Result<HashMap<i32, NaifCelestial>, String> {
        match coord_system {
            CoordSystem::BodyCentric => Simulation::init_body_centric(time_et, ref_frame, available_satellites_ids),
            CoordSystem::BarocenterCentric => Simulation::init_barocenter_centric(time_et, ref_frame, available_satellites_ids),
        }
    }

    fn initialize_small_bodies(coord_system: &CoordSystem, time_et: f64, _ref_frame: &'static str, naif_map: &HashMap<i32, NaifCelestial>) -> HashMap<i32, SbCelestial> {
        let mut res: HashMap<i32, SbCelestial> = HashMap::new();

        let sun_pos = match coord_system {
            CoordSystem::BodyCentric => Vec3d::new(0.0, 0.0, 0.0),
            CoordSystem::BarocenterCentric => {
                if let Some(s) = naif_map.get(&10) { s.glob_state.position } else { Vec3d::new(0.0, 0.0, 0.0) }
            }
        };

        if let Ok(ids) = crate::modules::sbdb::downloaded_ids() {
            for id in ids {
                match SbCelestial::try_new(id, time_et, coord_system, Some(sun_pos)) {
                    Ok(n) => { res.insert(id, n); },
                    Err(e) => { info!("Skipping small-body id {}: {}", id, e); }
                }
            }
        }

        res
    }

    pub fn initialize_instances(&mut self) { // have to run manually after init
        // Clear previous instance lists before rebuilding to ensure GPU buffers
        // are recreated with correct sizes and avoid write overruns.
        self.orbit_instances.clear();
        self.celestial_marker_instances.clear();
        self.body_instances.clear();

        self.build_orbit_instances(self.render_origin());
        self.build_celestial_marker_instances(self.render_origin());
        self.build_bodies_instances(self.render_origin());
    } 

    fn init_barocenter_centric(time_et: f64, ref_frame: &'static str, available_satellites_ids: &Vec<i32>) -> Result<HashMap<i32, NaifCelestial>, String> {
        let mut celestials = HashMap::<i32, NaifCelestial>::new();

        let ssb_name = utils::naif_obj_name(0).ok_or_else(|| "SSB id does not exist in kernels".to_string())?;
        let ssb = NaifCelestial {
            id: 0,
            name: ssb_name,
            is_physical_body: false,
            physical_params: None,
                kep_elts: None,
                kep_primary_id: None,
            rel_state: StateVector::default(),
            glob_state: StateVector::default(),
            parent_id: None,
            constituents_ids: vec![],
        };
        celestials.insert(0, ssb);

        let sun_name = utils::naif_obj_name(10).ok_or_else(|| "Sun id does not exist in kernels".to_string())?;
        let sun_radii = utils::radii(10).map_err(|e| format!("could not get sun radius from kernels: {}", e))?;
        let sun_rel_state = utils::celestial_state_et(time_et, 10, 0, &ref_frame).map_err(|e| format!("Could not get sun rel state to SSB: {}", e))?;
        let sun_mu = utils::mu(10).map_err(|e| format!("could not get sun mu from kernels: {}", e))?;
        let sun = NaifCelestial {
            id: 10,
            name: sun_name,
            is_physical_body: true,
            physical_params: Some(PhysicalParams { radii: sun_radii, mu: sun_mu }),
            kep_elts: None,
            rel_state: sun_rel_state,
            kep_primary_id: None,
            glob_state: sun_rel_state,
            parent_id: Some(0),
            constituents_ids: vec![],
        };
        celestials.insert(10, sun);

        if let Some(ssb) = celestials.get_mut(&0) {
            ssb.constituents_ids.push(10);
        }

        for i in 1..=9 {
            info!("Initializing barycenter for planet {}", i);
            let bary_id = i;
            let bary_name = match utils::naif_obj_name(bary_id) {
                Some(n) => n,
                None => { info!("Barycenter {} not found, skipping", bary_id); continue; }
            };
            let bary_rel_state = match utils::celestial_state_et(time_et, bary_id, 0, &ref_frame) {
                Ok(s) => s,
                Err(_) => { info!("Could not get rel state for barycenter {}, skipping", bary_id); continue; }
            };

            let mut barycenter = NaifCelestial {
                id: bary_id,
                name: bary_name,
                is_physical_body: false,
                physical_params: None,
                kep_elts: None,
                rel_state: bary_rel_state,
                kep_primary_id: None,
                glob_state: bary_rel_state, 
                parent_id: Some(0),
                constituents_ids: vec![],
            };

            let planet_id = i * 100 + 99;
            let mut ids: Vec<i32> = available_satellites_ids.iter()
                .filter(|&&id| barycenter.valid_sat_id(id, &CoordSystem::BarocenterCentric))
                .map(|&id| id)
                .collect();
            ids.push(planet_id);

            info!("Calling populate_planetary_system for {}", barycenter.name);
            let populated_system = barycenter.populate_planetary_system(&CoordSystem::BarocenterCentric, time_et, &ref_frame, Some(ids));
            celestials.insert(bary_id, barycenter);
            celestials.extend(populated_system);

            if let Some(ssb) = celestials.get_mut(&0) {
                ssb.constituents_ids.push(bary_id);
            }
        }

        Ok(celestials)
    }

    fn init_body_centric(time_et: f64, ref_frame: &'static str, available_satellites_ids: &Vec<i32>) -> Result<HashMap<i32, NaifCelestial>, String> {
        let mut celestials = HashMap::<i32, NaifCelestial>::new();
        let sn: String = utils::naif_obj_name(10).ok_or_else(|| "Sun id does not exist in kernels".to_string())?;
        
        let sun_radii = utils::radii(10).map_err(|e| format!("could not get sun radius from kernels: {}", e))?;

        let sun_mu = utils::mu(10).map_err(|e| format!("could not get sun mu from kernels: {}", e))?;

        let sun = NaifCelestial {
            id: 10,
            name: sn,
            is_physical_body: true,
            physical_params: Some(PhysicalParams{radii: sun_radii, mu: sun_mu}),
            kep_elts: None,
                rel_state: StateVector::default(),
                kep_primary_id: None,
            glob_state: StateVector::default(),
            parent_id: None,
            constituents_ids: vec![],
        };
        celestials.insert(10, sun);

        let ssb_n: String = utils::naif_obj_name(0).ok_or_else(|| "SSB id does not exist in kernels".to_string())?;
        let ssb_state = utils::celestial_state_et(time_et, 0, 10, &ref_frame).map_err(|e| format!("Could not get ssb rel state: {}", e))?;

        let ssb = NaifCelestial {
            id: 0,
            name: ssb_n,
            is_physical_body: false,
            physical_params: None,
            kep_elts: None,
                rel_state: ssb_state,
                kep_primary_id: None,
            glob_state: ssb_state,
            parent_id: Some(10),
            constituents_ids: vec![],
        };
        celestials.insert(0, ssb);

        for i in 1..=9 {
            info!("Initializing celestial for planet {}", i);
            let planet_id = i*100+99;

            let mut planet = match NaifCelestial::try_new(planet_id, 10, time_et, &ref_frame, &CoordSystem::BodyCentric) {
                Ok(p) => p,
                Err(e) => { info!("Planet {} not found or error: {}", planet_id, e); continue; }
            };

            info!("Calling populate_planetary_system for {}", planet.name);

            let ids = available_satellites_ids.iter()
                .filter(|id| planet.valid_sat_id(**id, &CoordSystem::BodyCentric))
                .map(|id| *id)
                .collect::<Vec<i32>>();
            
            let inner_planets = planet.populate_planetary_system(&CoordSystem::BodyCentric, time_et, &ref_frame, Some(ids));//
            celestials.insert(planet_id, planet);
            celestials.extend(inner_planets);

            if let Some(planet) = celestials.get_mut(&planet_id) {
                planet.constituents_ids.push(i);
            }

            let planet_barycenter_name: String = match utils::naif_obj_name(i) {
                Some(n) => n,
                None => { info!("Planetary barycenter {} not found, skipping barycenter", i); continue; }
            };
            let barycenter_rel_state = match utils::celestial_state_et(time_et, i, planet_id, &ref_frame) {
                Ok(s) => s,
                Err(_) => { info!("Could not get rel state for barycenter {}, skipping", i); continue; }
            };
            let barycenter_glob_state = match utils::celestial_state_et(time_et, i, 10, &ref_frame) {
                Ok(s) => s,
                Err(_) => { info!("Could not get glob state for barycenter {}, skipping", i); continue; }
            };

            let barycenter = NaifCelestial {
                id: i,
                name: planet_barycenter_name,
                is_physical_body: false,
                physical_params: None,
                kep_elts: None,
                    rel_state: barycenter_rel_state,
                    kep_primary_id: None,
                glob_state: barycenter_glob_state,
                parent_id: Some(planet_id),
                constituents_ids: vec![],
            };
            celestials.insert(i, barycenter);

        }

        Ok(celestials)
    }

    pub fn update(&mut self, dt: std::time::Duration) {

        if !self.paused {
            let dt_seconds = dt.as_secs_f64() * self.time_speed;
            if dt_seconds != 0.0 {
                self.current_time_et += dt_seconds;
                self.current_time_et = self.current_time_et.clamp(self.min_time_et, self.max_time_et);
                self.current_time_utc = utils::et_to_utc(self.current_time_et)
                    .unwrap_or_else(|_| "Invalid Time".to_string());

                let sun_pos = match self.coord_system {
                    CoordSystem::BodyCentric => Vec3d::new(0.0, 0.0, 0.0),
                    CoordSystem::BarocenterCentric => {
                        if let Some(s) = self.naif_celestial_objects.get(&10) { s.glob_state.position } else { Vec3d::new(0.0,0.0,0.0) }
                    }
                };

                for sb in self.sb_celestial_objects.values_mut() {
                    if let Some(integrator) = &self.integrator && integrator.sb_id == sb.id {
                        if self.current_time_et < integrator.start_time || self.current_time_et > integrator.end_time { 
                            self.paused = true;
                        } else {
                            sb.update_integrated(integrator.state_at(self.current_time_et));
                        }
                    } else {
                        sb.update(self.current_time_et, Some(sun_pos));
                    }
                }

                for celestial in Arc::make_mut(&mut self.naif_celestial_objects).values_mut() {
                    celestial.update(self.current_time_et, self.ref_frame, &self.coord_system);
                }
                if self.time_speed > 1.0 || (self.upd_kep_elts && self.time_speed == 1.0) {
                    for celestial in Arc::make_mut(&mut self.naif_celestial_objects).values_mut() {
                        celestial.update_kep_elts(self.current_time_et, self.ref_frame);
                    }
                    self.upd_kep_elts = false;
                }
                
            }
        }
    }

    pub fn add_small_body(&mut self, id: i32) -> Result<(), String> {
        if self.sb_celestial_objects.contains_key(&id) {
            return Ok(());
        }

        let sun_pos = match self.coord_system {
            CoordSystem::BodyCentric => Vec3d::new(0.0, 0.0, 0.0),
            CoordSystem::BarocenterCentric => {
                if let Some(s) = self.naif_celestial_objects.get(&10) {
                    s.glob_state.position
                } else {
                    Vec3d::new(0.0, 0.0, 0.0)
                }
            }
        };

        match SbCelestial::try_new(id, self.current_time_et, &self.coord_system, Some(sun_pos)) {
            Ok(n) => {
                self.sb_celestial_objects.insert(id, n);
                Self::insert_sorted_unique(&mut self.sb_sorted_ids, id);
                self.sb_filter.insert(id);
                self.sb_orbit_filter.insert(id);
                Ok(())
            }
            Err(e) => {
                let _ = crate::modules::sbdb::delete_downloaded_small_bodies(&[id]);
                Err(format!("{}", e))
            }
        }
    }

    /// remove a set of small-body objects by ID and clear any filter/selection
    /// state for them. Caller is responsible for rebuilding GPU instances.
    pub fn remove_small_bodies(&mut self, ids: &[i32]) {
        for id in ids {
            self.sb_celestial_objects.remove(id);
            Self::remove_sorted(&mut self.sb_sorted_ids, *id);
            self.sb_filter.remove(id);
            self.sb_orbit_filter.remove(id);

            if self.focused_body_type == FocusedBodyType::SmallBody && self.focused_body_id == *id {
                self.focused_body_type = FocusedBodyType::Naif;
                self.focused_body_id = 10;
            }
        }
    }

    fn build_orbit_instances(&mut self, origin: Vec3d) {

        for id in self.naif_sorted_ids.iter() {
            let Some(celestial) = self.naif_celestial_objects.get(id) else { continue };
            let Some(kep) = celestial.kep_elts else { continue };

            if kep.ecc >= 1.0 { continue; }

            if !kep.a.is_finite() || kep.a <= 0.0 { continue; }

            let primary_id = match celestial.kep_primary_id {
                Some(pid) => pid,
                None => continue,
            };

            let primary = match self.naif_celestial_objects.get(&primary_id) {
                Some(p) => p,
                None => continue,
            };

            let rotation = convert_quat_f64_to_f32(kep.rotation);

            // f1, f2 - foci of the ellipse, c - distance from center to focus, a - semi-major axis, b - semi-minor axis
            // ellipse was centered ( f1  x  f2 )
            // we have to offset by c to focus:                  
            //  ( f1 <-c-> x <-c-> f2)

            let local_offset = cgmath::Vector3::new(-(kep.c as f32), 0.0, 0.0);
            let world_offset = rotation.rotate_vector(local_offset);

            let position = cgmath::Vector3::new(
                (primary.glob_state.position.x - origin.x) as f32,
                (primary.glob_state.position.y - origin.y) as f32,
                (primary.glob_state.position.z - origin.z) as f32,
            ) + world_offset;
            let scale = cgmath::Vector3::new(kep.a as f32, kep.b as f32, 1.0);

            let instance = Instance { position, rotation, scale, color: get_celestial_color(celestial.id), id: Some(celestial.id), is_sbdb: false };

            self.orbit_instances.push(instance);
        }
        
        // Small-body orbits
        for sb in self.sb_celestial_objects.values() {
            let kep = &sb.kep_elts;
            if kep.ecc >= 1.0 { continue; }
            if !kep.a.is_finite() || kep.a <= 0.0 { continue; }
            // primary is Sun (10)
            let primary = match self.naif_celestial_objects.get(&10) {
                Some(p) => p,
                None => continue,
            };
            let rotation = convert_quat_f64_to_f32(kep.rotation);
            let local_offset = cgmath::Vector3::new(-(kep.c as f32), 0.0, 0.0);
            let world_offset = rotation.rotate_vector(local_offset);
            let position = cgmath::Vector3::new(
                (primary.glob_state.position.x - origin.x) as f32,
                (primary.glob_state.position.y - origin.y) as f32,
                (primary.glob_state.position.z - origin.z) as f32,
            ) + world_offset;
            let scale = cgmath::Vector3::new(kep.a as f32, kep.b as f32, 1.0);
            let instance = Instance { position, rotation, scale, color: get_celestial_color(sb.id), id: Some(sb.id), is_sbdb: true };
            self.orbit_instances.push(instance);
        }
    }

    fn build_celestial_marker_instances(&mut self, origin: Vec3d) {
        for celestial in self.naif_celestial_objects.values() {
            let position = cgmath::Vector3::new(
                (celestial.glob_state.position.x - origin.x) as f32,
                (celestial.glob_state.position.y - origin.y) as f32,
                (celestial.glob_state.position.z - origin.z) as f32,
            );
            
            let rotation = cgmath::Quaternion::new(1.0, 0.0, 0.0, 0.0); // identity
            let scale = cgmath::Vector3::new(1.0, 1.0, 1.0);

            let instance = Instance { position, rotation, scale, color: get_celestial_color(celestial.id), id: Some(celestial.id), is_sbdb: false };
            self.celestial_marker_instances.push(instance);
        }

        // Small-body markers
        for sb in self.sb_celestial_objects.values() {
            let position = cgmath::Vector3::new(
				(sb.glob_state.position.x - origin.x) as f32,
				(sb.glob_state.position.y - origin.y) as f32,
				(sb.glob_state.position.z - origin.z) as f32,
            );
            let rotation = cgmath::Quaternion::new(1.0, 0.0, 0.0, 0.0);
            let scale = cgmath::Vector3::new(1.0, 1.0, 1.0);
            let instance = Instance { position, rotation, scale, color: get_celestial_color(sb.id), id: Some(sb.id), is_sbdb: true };
            self.celestial_marker_instances.push(instance);
        }

    }

    fn build_bodies_instances(&mut self, origin: Vec3d) {
        for celestial in self.naif_celestial_objects.values() {
            if !celestial.is_physical_body || celestial.id == 10 || spice_ker::naif_id_is_barycenter(celestial.id) {
                continue;
            }

            let position = cgmath::Vector3::new(
                (celestial.glob_state.position.x - origin.x) as f32,
                (celestial.glob_state.position.y - origin.y) as f32,
                (celestial.glob_state.position.z - origin.z) as f32,
            );

            let radius = match &celestial.physical_params {
                Some(params) => params.radius() as f32,
                None => 1.0,
            };

            let rotation = cgmath::Quaternion::new(1.0, 0.0, 0.0, 0.0); // identity
            let scale = cgmath::Vector3::new(radius, radius, radius);

            let instance = Instance { position, rotation, scale, color: get_celestial_color(celestial.id), id: Some(celestial.id), is_sbdb: false };
            self.body_instances.push(instance);
        }

        // Small bodies
        for sb in self.sb_celestial_objects.values() {
            let position = cgmath::Vector3::new(
                (sb.glob_state.position.x - origin.x) as f32,
                (sb.glob_state.position.y - origin.y) as f32,
                (sb.glob_state.position.z - origin.z) as f32,
            );
			let radius = sb.physical_params
				.as_ref()
				.map(|p| p.radius() as f32)
				.unwrap_or(1.0);
            let rotation = cgmath::Quaternion::new(1.0, 0.0, 0.0, 0.0);
            let scale = cgmath::Vector3::new(radius, radius, radius);
            let instance = Instance { position, rotation, scale, color: get_celestial_color(sb.id), id: Some(sb.id), is_sbdb: true };
            self.body_instances.push(instance);
        }

    }

    pub fn current_time_utc(&self) -> &str {
        &self.current_time_utc
    }

    pub fn set_time(&mut self, utc: &str) -> Result<(), String> {
        match utils::utc_to_et(utc) {
            Ok(et) => {
                self.current_time_et = et;
                self.current_time_utc = utils::et_to_utc(et).unwrap_or_else(|_| utc.to_string());
                
                for celestial in Arc::make_mut(&mut self.naif_celestial_objects).values_mut() {
                    celestial.update(self.current_time_et, self.ref_frame, &self.coord_system);
                    celestial.update_kep_elts(self.current_time_et, self.ref_frame);
                }
            
                let sun_pos = match self.coord_system {
                    CoordSystem::BodyCentric => Vec3d::new(0.0, 0.0, 0.0),
                    CoordSystem::BarocenterCentric => {
                        if let Some(s) = self.naif_celestial_objects.get(&10) { s.glob_state.position } else { Vec3d::new(0.0,0.0,0.0) }
                    }
                };
                for sb in self.sb_celestial_objects.values_mut() {
                    if let Some(integrator) = &self.integrator && integrator.sb_id == sb.id {
                        sb.update_integrated(integrator.state_at(self.current_time_et));
                    } else {
                        sb.update(self.current_time_et, Some(sun_pos));
                    }
                }
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn sun_light_uniform(&self, origin: Vec3d) -> crate::modules::projection_3d::window_core::light::LightUniform {

        if let Some(sun) = self.naif_celestial_objects.get(&10) {
            let glob_state = sun.glob_state;
            let scale = match sun.physical_params {
                Some(p) => p.radius() as f32,
                None => 1.0,
            };
            let pos_vec = cgmath::Vector3::new(
                (glob_state.position.x - origin.x) as f32,
                (glob_state.position.y - origin.y) as f32,
                (glob_state.position.z - origin.z) as f32,
            );

            crate::modules::projection_3d::window_core::light::LightUniform {
                position: [pos_vec.x, pos_vec.y, pos_vec.z],
                _padding: 0,
                color: [1.0, 1.0, 0.9],
                scale,
            }
        } else {
            crate::modules::projection_3d::window_core::light::LightUniform {
                position: [0.0, 0.0, 0.0],
                _padding: 0,
                color: [1.0, 1.0, 1.0],
                scale: 1.0,
            }
        }
    }

    // update instances positions based on new render origin (ro) and cel states
    pub fn upd_ins_pos_ro(&mut self, new_render_origin: Vec3d) {
        // Update marker instances positions
        for instance in self.celestial_marker_instances.iter_mut() {
            if let Some(id) = instance.id {
                if let Some(celestial) = self.naif_celestial_objects.get(&id) {
                    instance.position = cgmath::Vector3::new(
                        (celestial.glob_state.position.x - new_render_origin.x) as f32,
                        (celestial.glob_state.position.y - new_render_origin.y) as f32,
                        (celestial.glob_state.position.z - new_render_origin.z) as f32,
                    );
                } else if let Some(sb) = self.sb_celestial_objects.get(&id) {
                    instance.position = cgmath::Vector3::new(
                        (sb.glob_state.position.x - new_render_origin.x) as f32,
                        (sb.glob_state.position.y - new_render_origin.y) as f32,
                        (sb.glob_state.position.z - new_render_origin.z) as f32,
                    );
                }
            }
        }

        // Update body instances positions
        for instance in self.body_instances.iter_mut() {
            if let Some(id) = instance.id {
                if let Some(celestial) = self.naif_celestial_objects.get(&id) {
                    if celestial.is_physical_body && celestial.id != 10 && !spice_ker::naif_id_is_barycenter(celestial.id) {
                        instance.position = cgmath::Vector3::new(
                            (celestial.glob_state.position.x - new_render_origin[0]) as f32,
                            (celestial.glob_state.position.y - new_render_origin[1]) as f32,
                            (celestial.glob_state.position.z - new_render_origin[2]) as f32,
                        );
                    }
                } else if let Some(sb) = self.sb_celestial_objects.get(&id) {
                    // Small bodies are physical bodies; update their positions as well
                    instance.position = cgmath::Vector3::new(
                        (sb.glob_state.position.x - new_render_origin[0]) as f32,
                        (sb.glob_state.position.y - new_render_origin[1]) as f32,
                        (sb.glob_state.position.z - new_render_origin[2]) as f32,
                    );
                }
            }
        }

        // Update closest approach marker positions
        for (i, global_point) in self.closest_approach_points.iter().enumerate() {
            if let Some(instance) = self.closest_approach_instances.get_mut(i) {
                let approach_event_id = instance.id.map(|id| (id - 10_000_000) as u32);
                if let Some(integrator) = &self.integrator
                    && let Some(event_id) = approach_event_id
                    && integrator.closest_approaches_ref().get(i).map(|approach| approach.body_id) == Some(self.focused_body_id)
                    && let Some(planet) = self.naif_celestial_objects.get(&self.focused_body_id)
                    && let Some(closest_approach_et) = integrator.closest_approach_et_for_event(event_id)
                    && let Some(tca_planet_pos) = planet.glob_pos_at_time(closest_approach_et, "ECLIPJ2000", &self.coord_system)
                {
                    let offset = [
                        global_point[0] - tca_planet_pos[0],
                        global_point[1] - tca_planet_pos[1],
                        global_point[2] - tca_planet_pos[2],
                    ];
                    // new_render_origin is the current focused-body position,
                    // so the marker can be placed relative to the focused body.
                    instance.position = cgmath::Vector3::new(
                        offset[0] as f32,
                        offset[1] as f32,
                        offset[2] as f32,
                    );
                } else {
                    instance.position = cgmath::Vector3::new(
                        (global_point[0] - new_render_origin[0]) as f32,
                        (global_point[1] - new_render_origin[1]) as f32,
                        (global_point[2] - new_render_origin[2]) as f32,
                    );
                }
            }
        }

        // Update orbit instances 
        // MARK: have to upd every frame 
        // orbits are centered on primary, so we compute primary position based on new render origin and add the offset to get the new position of the orbit center

        // except for SBs - TODO: could be optimized by keeping track of which orbit instances correspond to SBs and which to Naif celestials instead of iterating over all for each type, but this is simpler for now and the number of SB orbits is not huge
        let mut orbit_iter = self.orbit_instances.iter_mut();
        for id in self.naif_sorted_ids.iter() {
            let Some(celestial) = self.naif_celestial_objects.get(id) else { continue };
            let Some(kep) = celestial.kep_elts else { continue };
            if kep.ecc >= 1.0 { continue; }
            if !kep.a.is_finite() || kep.a <= 0.0 { continue; }
            let Some(primary_id) = celestial.kep_primary_id else { continue };
            let Some(primary) = self.naif_celestial_objects.get(&primary_id) else { continue };

            let rotation = convert_quat_f64_to_f32(kep.rotation);

            let local_offset = cgmath::Vector3::new(-(kep.c as f32), 0.0, 0.0);
            let world_offset = rotation.rotate_vector(local_offset);

            if let Some(instance) = orbit_iter.next() {
                instance.position = cgmath::Vector3::new(
                    (primary.glob_state.position.x - new_render_origin[0]) as f32,
                    (primary.glob_state.position.y - new_render_origin[1]) as f32,
                    (primary.glob_state.position.z - new_render_origin[2]) as f32,
                ) + world_offset;
                instance.rotation = rotation;
                instance.scale = cgmath::Vector3::new(kep.a as f32, kep.b as f32, 1.0);
            }
        }

        // sb orbit instances update: center orbits on primary (Sun) like when built
        for sb in self.sb_celestial_objects.values() {
            let kep = &sb.kep_elts;
            if kep.ecc >= 1.0 { continue; }
            if !kep.a.is_finite() || kep.a <= 0.0 { continue; }
            let rotation = convert_quat_f64_to_f32(kep.rotation);
            let local_offset = cgmath::Vector3::new(-(kep.c as f32), 0.0, 0.0);
            let world_offset = rotation.rotate_vector(local_offset);
               
            if let Some(primary) = self.naif_celestial_objects.get(&10) {
                if let Some(instance) = orbit_iter.next() {
                    instance.position = cgmath::Vector3::new(
                        (primary.glob_state.position.x - new_render_origin[0]) as f32,
                        (primary.glob_state.position.y - new_render_origin[1]) as f32,
                        (primary.glob_state.position.z - new_render_origin[2]) as f32,
                    ) + world_offset;
                    instance.rotation = rotation;
                    instance.scale = cgmath::Vector3::new(kep.a as f32, kep.b as f32, 1.0);
                }
            }
        }
    }

}
