#![allow(non_snake_case)]

use std::{collections::HashMap, fmt};
use cgmath::InnerSpace;
use log::{info, warn};
use crate::modules::projection_3d::simulation::CoordSystem;
use crate::modules::{sbdb, utils};
use anyhow::{Result, Context};
use super::traj::{PhysicalParams, KeplerianElts};
use super::state::{StateVector, Vec3d};
use std::write;

#[derive(Clone)]
pub struct NaifCelestial { 
	pub id: i32,  
	pub name: String,                
	pub is_physical_body: bool,
	pub physical_params: Option<PhysicalParams>,
	pub kep_elts: Option<KeplerianElts>,
	pub kep_primary_id: Option<i32>,
	pub rel_state: StateVector,    
	pub glob_state: StateVector,    
	pub parent_id: Option<i32>,
	pub constituents_ids: Vec<i32>        
}

impl NaifCelestial {
	/// Map barycenter observers to their physical primaries for Keplerian elements
	/// Takes target id into account to avoid degenerate cases (e.g., planet body under its barycenter)
	/// Rules:
	/// - 0 (SSB) -> 10 (Sun)
	/// - 1..=9 (planet barycenters):
	///     - if target is the planet body (id*100+99) -> 10 (Sun)
	///     - else -> id*100+99 (planet body)
	/// - otherwise unchanged
	pub fn observer_as_body_for(target_id: i32, observer: i32) -> i32 {
		if observer == 0 {
			return 10;
		}
		if (1..=9).contains(&observer) {
			let planet_body = observer * 100 + 99;
			if target_id == planet_body {
				10
			} else {
				planet_body
			}
		} else {
			observer
		}
	}

	pub fn populate_planetary_system(&mut self, coord_system: &CoordSystem, et: f64, ref_frame: &str, ids: Option<Vec<i32>>) -> HashMap<i32, NaifCelestial> { 

		info!("populate_planetary_system called for {}, (id: {}) et {}, ref_frame {}", self.name, self.id, et, ref_frame);
		let mut res: HashMap<i32, NaifCelestial> = HashMap::new();
		let is_barocenter_centric: bool;
		let benchmark_id: i32;
		match coord_system {
			CoordSystem::BodyCentric => {
				benchmark_id = self.id - 99;
				is_barocenter_centric = false;
			},
			CoordSystem::BarocenterCentric => {
				benchmark_id = self.id * 100;
				is_barocenter_centric = true;
			},
		}	

		if benchmark_id % 100 != 0 {return res;}

		match ids {
			Some(id_list) => {
				for id in &id_list {
					match NaifCelestial::try_new(*id, self.id, et, ref_frame, coord_system) {
						Ok(b) => {
							self.constituents_ids.push(*id);
							res.insert(*id, b);
						},
						Err(e) => { info!("Skipping id {}: {}", id, e); continue }
					}
				}
				if is_barocenter_centric && !id_list.contains(&(benchmark_id + 99)) {
					match NaifCelestial::try_new(benchmark_id + 99, self.id, et, ref_frame, coord_system) {
						Ok(b) => {
							self.constituents_ids.push(benchmark_id + 99);
							res.insert(benchmark_id + 99, b);
						},
						Err(_) => {}
					}
				}	
			},
			None => {
				if is_barocenter_centric {
					res.extend(self.check_all(et, ref_frame, benchmark_id, 99, coord_system));
				} else {
					res.extend(self.check_all(et, ref_frame, benchmark_id, 98, coord_system));	
				}
				res.extend(self.check_all(et, ref_frame, benchmark_id * 100, 999, coord_system));
				res.extend(self.check_all(et, ref_frame, (benchmark_id * 100) + 5000, 999, coord_system));
			}
		}
	
		res
	}

	fn check_all(&mut self, et: f64, ref_frame: &str, benchmark_id: i32, max_iter: i32, coord_system: &CoordSystem) -> HashMap<i32, NaifCelestial> {
		let mut res: HashMap<i32, NaifCelestial> = HashMap::new();

		for i in 1..=max_iter {
			
			let body_id = benchmark_id + i;
			
			match NaifCelestial::try_new(body_id, self.id, et, ref_frame, coord_system) {
				Ok(b) => {
					self.constituents_ids.push(body_id);
					res.insert(body_id, b);
				},
				Err(_) => continue
			}
		}
		res
	}

	pub fn try_new(id: i32, observer: i32, et: f64, ref_frame: &str, coord_system: &CoordSystem) -> Result<NaifCelestial> {
		let body_name = match utils::naif_obj_name(id) {
			Some(name) => name,
			None => {
				warn!("No name in kernels for id: {}", id);
				id.to_string()
			},
		};

		let rel_body_state = utils::celestial_state_et(et, id, observer, ref_frame)
			.with_context(|| format!("could not get relative state for id {} observer {}", id, observer))?;

		let central = match coord_system {
			CoordSystem::BodyCentric => 10,
			CoordSystem::BarocenterCentric => 0,
		};

		let glob_body_state = utils::celestial_state_et(et, id, central, ref_frame)
			.with_context(|| format!("could not get global state for id {} central {}", id, central))?;

		// Compute Keplerian elements with respect to a physical primary, not barycenters
		let kep_observer = NaifCelestial::observer_as_body_for(id, observer);
		let kep_state = match utils::celestial_state_et(et, id, kep_observer, ref_frame) {
			Ok(s) => s,
			Err(_) => rel_body_state, // fallback to existing rel state
		};

		let radii = utils::radii(id).with_context(|| format!("missing radii for id {}", id))?;
		let mu = utils::mu(id).unwrap_or(0.0);
		// MISSING: IRREGULAR satellites may have 0. but make sure we account for that in the integrator.

		let kep_elts = match utils::mu(kep_observer) {
			Ok(gm) => {
				info!("Computing Keplerian elements for body id {} with observer {} at et {}", id, kep_observer, et);
				// Check if state is a zero vector (degenerate case)
				let state_magnitude = kep_state.position.magnitude();
				if state_magnitude < 1e-10 {
					warn!("ZERO VECTOR detected for body id {} observer {} at et {} - skipping Keplerian elements (state_mag={})", 
						id, kep_observer, et, state_magnitude);
					None
				} else {
					match utils::keplerian_elements(&kep_state, et, gm) {
						Ok(elts_arr) => Some(KeplerianElts::from_array(elts_arr, et)),
						Err(error) => {
							warn!("Could not compute Keplerian elements for body {}: {}", id, error);
							None
						}
					}
				}
			},
			Err(e) => {
				warn!("Could not get mu for keplerian elements computation: body_id={}, observer={}, error={}", id, kep_observer, e);
				None
			},
		};

		Ok(NaifCelestial {
			id,
			name: body_name,
			is_physical_body: true,
			physical_params: Some(PhysicalParams{ radii, mu }),
			kep_elts,
			kep_primary_id: Some(kep_observer),
			rel_state: rel_body_state,
			glob_state: glob_body_state,
			parent_id: Some(observer),
			constituents_ids: vec![],
		})
	}

	pub fn update_kep_elts(&mut self, et: f64, ref_frame: &str) {
		if let Some(kep) = &mut self.kep_elts {
			if let Some(primary_id) = self.kep_primary_id {
				if let Ok(state) = utils::celestial_state_et(et, self.id, primary_id, ref_frame) {
					if let Ok(gm) = utils::mu(primary_id) {
						info!("Updating Keplerian elements for body id {} with primary {} at et {}", self.id, primary_id, et);
						// Check for zero vector before calling SPICE
						let state_magnitude = state.position.magnitude();
						if state_magnitude < 1e-10 {
							warn!("ZERO VECTOR detected during update for body id {} primary {} at et {} - skipping update (state_mag={})", 
								self.id, primary_id, et, state_magnitude);
						} else {
							if let Ok(elts_arr) = utils::keplerian_elements(&state, et, gm) {
								*kep = KeplerianElts::from_array(elts_arr, et);
							}
						}
					} else {
						warn!("Could not get mu for primary {} when updating keplerian elements for body {}", primary_id, self.id);
					}
				} else {
					warn!("Could not get state for body {} relative to primary {} at et {}", self.id, primary_id, et);
				}
			}
		}
	}

	pub fn valid_sat_id(&self, id: i32, coord_system: &CoordSystem) -> bool { 
		let benchmark_id = match coord_system {
			CoordSystem::BodyCentric => self.id - 99,
			CoordSystem::BarocenterCentric => self.id * 100,
		};
		
		if benchmark_id % 100 != 0 {return false;}
		
		(id > benchmark_id && id < benchmark_id + 99) ||
		(id > benchmark_id * 100 && id < (benchmark_id * 100) + 999) ||
		(id > (benchmark_id * 100) + 5000 && id < (benchmark_id * 100) + 5999)
	}

	pub fn update(&mut self, et: f64, ref_frame: &str, coord_system: &CoordSystem) {
		// relative state (relative to parent)
		if let Some(parent_id) = self.parent_id {
			if let Ok(rel_state) = utils::celestial_state_et(et, self.id, parent_id, ref_frame) {
				self.rel_state = rel_state;
			}
		}

		// global state (relative to central body)
		let central = match coord_system {
			CoordSystem::BodyCentric => 10,
			CoordSystem::BarocenterCentric => 0,
		};

		if let Ok(glob_state) = utils::celestial_state_et(et, self.id, central, ref_frame) {
			self.glob_state = glob_state;
		}

	}

	pub fn hill_sphere_radius(&self) -> Option<f64> {
		if (self.id - 99) % 100 != 0 {
			return None;
		}

		let sun_mu = utils::mu(10).ok()?;

		let phys_params = self.physical_params.as_ref()?;
		if phys_params.mu <= 0.0 {
			return None;
		}
		let kep_elts = self.kep_elts.as_ref()?;

		let a = kep_elts.rp / (1.0 - kep_elts.ecc);

		let mass_ratio = phys_params.mu / sun_mu;
		let r_h = a * (mass_ratio / 3.0).powf(1.0 / 3.0);

		Some(r_h)
	}

	pub fn soi_radius(&self) -> Option<f64> {
		if (self.id - 99) % 100 != 0 {
			return None;
		}

		let sun_mu = utils::mu(10).ok()?;

		let phys_params = self.physical_params.as_ref()?;
		if phys_params.mu <= 0.0 {
			return None;
		}
		let kep_elts = self.kep_elts.as_ref()?;

		let a = kep_elts.rp / (1.0 - kep_elts.ecc);

		let mass_ratio = phys_params.mu / sun_mu;
		let r_soi = a * mass_ratio.powf(2.0 / 5.0);

		Some(r_soi)
	}

	pub fn glob_pos_at_time(&self, et: f64, ref_frame: &str, coord_system: &CoordSystem) -> Option<Vec3d> {
		let central = match coord_system {
			CoordSystem::BodyCentric => 10,
			CoordSystem::BarocenterCentric => 0,
		};
		utils::celestial_state_et(et, self.id, central, ref_frame).ok().map(|state| state.position)
	}

	pub fn glob_state_at_time(&self, et: f64, ref_frame: &str, coord_system: &CoordSystem) -> Option<StateVector> {
		let central = match coord_system {
			CoordSystem::BodyCentric => 10,
			CoordSystem::BarocenterCentric => 0,
		};
		utils::celestial_state_et(et, self.id, central, ref_frame).ok()
	}

}

impl fmt::Display for NaifCelestial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "NaifCelestial {{\n\
            \x20\x20id: {},\n\
            \x20\x20name: \"{}\",\n\
            \x20\x20is_physical: {},\n\
            \x20\x20physical_params: {:#?},\n\
            \x20\x20rel_state: {:#?},\n\
            \x20\x20glob_state: {:#?},\n\
            \x20\x20parent_id: {:?},\n\
            \x20\x20constituents: {:?}\n\
            }}",
            self.id,
            self.name,
            self.is_physical_body,
            self.physical_params,
            self.rel_state,
            self.glob_state,
            self.parent_id,
            self.constituents_ids
        )
    }
}

pub struct SbCelestial {
	pub id: i32,  
	pub name: String,             
	pub physical_params: Option<PhysicalParams>,
	pub kep_elts: KeplerianElts,
	pub last_E: f64,
	//pub kep_primary_id: Option<i32>, always 10 (Sun)
	pub glob_state: StateVector,    
	pub parent_id: i32,
}

impl SbCelestial {

	pub fn try_new(id: i32, et: f64, _coord_system: &CoordSystem, sun_pos: Option<Vec3d>) -> Result<SbCelestial> {
		let data = sbdb::get_celestial_data(id)
			.with_context(|| format!("Could not get SBDB data for id {}", id))?;

		let name = data
			.get("object")
			.and_then(|o| {
				o.get("fullname")
					.or_else(|| o.get("shortname"))
					.or_else(|| o.get("des"))
			})
			.and_then(|v| v.as_str())
			.map(|s| s.to_owned())
			.ok_or_else(|| anyhow::anyhow!("missing name for SBDB id {}", id))?;

		let mut radius_km: Option<f64> = None;
		let mut gm_km3_s2: Option<f64> = None;
		let mut h_mag: Option<f64> = None;
		let mut albedo: Option<f64> = None;

		if let Some(arr) = data.get("phys_par").and_then(|v| v.as_array()) {
			for entry in arr {
				let pname = entry
					.get("name")
					.and_then(|v| v.as_str())
					.unwrap_or("");
				let val_str = entry.get("value").and_then(|v| v.as_str());

				match pname {
					"diameter" => {
						if let Some(vs) = val_str {
							if let Ok(d_km) = vs.parse::<f64>() {
								radius_km = Some(d_km / 2.0);
							}
						}
					}
					"GM" => {
						if let Some(vs) = val_str {
							if let Ok(mu) = vs.parse::<f64>() {
								gm_km3_s2 = Some(mu);
							}
						}
					}
					"H" => {
						if let Some(vs) = val_str {
							if let Ok(h) = vs.parse::<f64>() {
								h_mag = Some(h);
							}
						}
					}
					"albedo" => {
						if let Some(vs) = val_str {
							if let Ok(pv) = vs.parse::<f64>() {
								albedo = Some(pv);
							}
						}
					}
					_ => {}
				}
			}
		}

		let computed_radius_km: Option<f64> = if let Some(r) = radius_km {
			Some(r)
		} else if let Some(h) = h_mag {
			let pv = albedo.unwrap_or(0.15);
			let d_m = sbdb::diameter_from_h_albedo_m(h, pv);
			Some(d_m / 2000.0)
		} else {
			None
		};

		let physical_params = if let Some(r) = computed_radius_km {
			Some(PhysicalParams {
				radii: [r; 3],
				mu: gm_km3_s2.unwrap_or(0.0),
			})
		} else if let Some(mu) = gm_km3_s2 {
			Some(PhysicalParams {
				radii: [1.0; 3],
				mu,
			})
		} else {
			None
		};

		let orbit = data
			.get("orbit")
			.ok_or_else(|| anyhow::anyhow!("missing orbit section for SBDB id {}", id))?;
		let elements = orbit
			.get("elements")
			.and_then(|v| v.as_array())
			.ok_or_else(|| anyhow::anyhow!("missing orbit.elements for SBDB id {}", id))?;

		let find_elem = |name: &str| {
			elements
				.iter()
				.find(|el| el.get("name").and_then(|v| v.as_str()) == Some(name))
		};

		let e = find_elem("e")
			.and_then(|el| el.get("value"))
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<f64>().ok())
			.ok_or_else(|| anyhow::anyhow!("missing/invalid eccentricity (e) for SBDB id {}", id))?;

		let a_au = find_elem("a")
			.and_then(|el| el.get("value"))
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<f64>().ok())
			.ok_or_else(|| anyhow::anyhow!("missing/invalid semi-major axis (a) for SBDB id {}", id))?;

		let inc_deg = find_elem("i")
			.and_then(|el| el.get("value"))
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<f64>().ok())
			.ok_or_else(|| anyhow::anyhow!("missing/invalid inclination (i) for SBDB id {}", id))?;

		let lnode_deg = find_elem("om")
			.and_then(|el| el.get("value"))
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<f64>().ok())
			.ok_or_else(|| anyhow::anyhow!("missing/invalid longitude of ascending node (om) for SBDB id {}", id))?;

		let argp_deg = find_elem("w")
			.and_then(|el| el.get("value"))
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<f64>().ok())
			.ok_or_else(|| anyhow::anyhow!("missing/invalid argument of perihelion (w) for SBDB id {}", id))?;

		let m0_deg = find_elem("ma")
			.and_then(|el| el.get("value"))
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<f64>().ok())
			.ok_or_else(|| anyhow::anyhow!("missing/invalid mean anomaly (ma) for SBDB id {}", id))?;

		let epoch_jd = orbit
			.get("epoch")
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<f64>().ok())
			.ok_or_else(|| anyhow::anyhow!("missing/invalid epoch JD for SBDB id {}", id))?;

		// AU -> km, degrees -> radians, JD -> ET seconds since J2000
		let a_km = a_au * super::simulation::AU_KM;
		let rp = a_km * (1.0 - e);
		let inc = inc_deg.to_radians();
		let lnode = lnode_deg.to_radians();
		let argp = argp_deg.to_radians();
		let m0 = m0_deg.to_radians();
		let epoch_et = utils::jd_to_et(epoch_jd)
			.with_context(|| format!("could not convert epoch JD for SBDB id {}", id))?;

		let elts_arr = [rp, e, inc, lnode, argp, m0];
		let kep = KeplerianElts::from_array(elts_arr, epoch_et);

		let sun_mu = utils::mu(10)
			.with_context(|| "Could not get Sun mu for SBDB small body creation")?;

		let (E, _v, _r) = kep.propagate_to_E_v_r(et, sun_mu, None);
		let mut state_vector = kep.state_from_eccentric_anomaly(E, sun_mu);

		if let Some(s) = sun_pos {
			state_vector.position += s;
		}

		Ok(SbCelestial {
			id,
			name,
			physical_params,
			kep_elts: kep,
			last_E: E,
			glob_state: state_vector,
			parent_id: 10,
		})

	}

	pub fn update(&mut self, et: f64, sun_pos: Option<Vec3d>) {
		let mu = match utils::mu(10) {
			Ok(m) => m,
			Err(_) => { warn!("Could not get Sun mu for small-body update"); return; }
		};
		let (E, _v, _r) = self.kep_elts.propagate_to_E_v_r(et, mu, Some(self.last_E));
		let mut state_vector = self.kep_elts.state_from_eccentric_anomaly(E, mu);
		if let Some(s) = sun_pos { state_vector.position += s; }
		self.glob_state = state_vector;
		self.last_E = E;
	}

	pub fn update_integrated(&mut self, new_state: StateVector) {
		self.glob_state = new_state;
	}	
}
