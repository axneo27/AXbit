#![allow(non_snake_case)]
use cgmath::num_traits::Pow;
use cgmath::{InnerSpace, MetricSpace, Quaternion, Rad, Rotation, Rotation3, Vector3};
use nalgebra::SVector;
use ode_solvers::{Dop853, OutputType, System};
use serde::{Deserialize, Serialize};
use log::warn;
use std::todo;
use super::{simulation, celestial, super::spice_ker};
use crate::modules::{utils, sbdb};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use super::pipelines::trajectory::DynamicTrajectorySegmentData;
use super::state::{StateVector, StateAtEpoch, Vec3d};

const EPS: f64 = 1e-12;
const STUMPF_C_MAX_ITER: usize = 30;
// const C_LIGHT: f64 = 299_792.458; // km/s
// const ONE_C2: f64 = 1.0 / (C_LIGHT * C_LIGHT); // 1/c^2, for relativity corrections

// Lookup table for 1/k!:
const FACTORIAL_INV: [f64; 20] = [
	1.0, // 1/0!
	1.0, // 1/1!
	0.5, // 1/2!
	0.16666666666666666, // 1/3!
	0.041666666666666664, // 1/4!
	0.008333333333333333, // 1/5!
	0.001388888888888889, // 1/6!
	0.0001984126984126984, // 1/7!
	2.48015873015873e-5, // 1/8!
	2.7557319223985893e-6, // 1/9!
	2.755731922398589e-7, // 1/10!
	2.505210838544172e-8, // 1/11!
	2.08767569878681e-9, // 1/12!
	1.6059043836821613e-10, // 1/13!
	1.1470745597729725e-11, // 1/14!
	7.647163731819816e-13, // 1/15!
	5.109094217170944e-14, // 1/16!
	3.556874280963844e-15, // 1/17!
	2.480914081139539e-16, // 1/18!
	1.5619206968586225e-17, // 1/19!
];

// MARK: A lot of hardcoded usage of ECLIPJ2000

#[derive(Copy, Clone, Debug)]
pub struct PhysicalParams {
	pub radii: [f64; 3],
	pub mu: f64
}

impl PhysicalParams {
	pub fn radius(&self) -> f64 {
		let non_zero_radii: Vec<f64> = self.radii.iter().filter(|&&r| r != 0.0).copied().collect();
		if non_zero_radii.is_empty() {
			1.0
		} else {
			non_zero_radii.iter().sum::<f64>() / non_zero_radii.len() as f64
		}
	}
}

#[derive(Copy, Clone, Debug)]
pub struct KeplerianElts {
	/// km
	pub rp: f64, 
	pub ecc: f64,
	/// rad
	pub inc: f64, 
	/// longitude of ascending node (rad)
	pub lnode: f64,
	/// argument of periapsis (rad)
	pub argp: f64, 
	/// at epoch (rad)
	pub m0: f64, 
	/// epoch (ET seconds)
	pub t0: f64, 
	pub a: f64,
	pub b: f64,
	pub c: f64,
	pub rotation: cgmath::Quaternion<f64>,
}

impl KeplerianElts {
	pub fn from_array(elts: [f64; 6], epoch_et: f64) -> Self {
		let rp = elts[0];
		let ecc = elts[1];
		let inc = elts[2];
		let lnode = elts[3];
		let argp = elts[4];
		let m0 = elts[5];

		let a = rp / (1.0 - ecc);
		let b = a * (1.0 - ecc * ecc).sqrt();
		let c = ecc * a;

		let q_raan = Quaternion::from_axis_angle(Vector3::unit_z(), Rad(lnode));
		let q_inc = Quaternion::from_axis_angle(Vector3::unit_x(), Rad(inc));
		let q_argp = Quaternion::from_axis_angle(Vector3::unit_z(), Rad(argp));
		let rotation = q_raan * q_inc * q_argp;

		KeplerianElts {
			rp,
			ecc,
			inc,
			lnode,
			argp,
			m0,
			t0: epoch_et,
			a,
			b,
			c,
			rotation,
		}
	}

	fn solve_eccentric_anomaly(m: f64, e: f64, prev_E: Option<f64>) -> f64 {
		// Normalize M to [-π, π]
		let mut M = m % (2.0 * std::f64::consts::PI);
		if M < -std::f64::consts::PI { M += 2.0 * std::f64::consts::PI; }
		if M > std::f64::consts::PI { M -= 2.0 * std::f64::consts::PI; }
		let mut E = match prev_E {
			Some(prev) => prev,
			None => {
				if e < 0.8 {
					M
				} else {
					// For high eccentricity, use atan2 formula for better convergence
					// E ≈ atan2(√(1-e²)·sin(M), e + cos(M))
					let sqrt_1_e2 = (1.0 - e * e).sqrt();
					(sqrt_1_e2 * M.sin()).atan2(e + M.cos())
				}
			}
		};
		
		// Fixed-point Newton-Raphson with cycle detection
		let mut E_prev1 = f64::NAN;
		let mut E_prev2;
		
		for iteration in 0..50 {
			E_prev2 = E_prev1;
			E_prev1 = E;
			
			let sin_E = E.sin();
			let cos_E = E.cos();
			let f = E - e * sin_E - M;
			let fp = 1.0 - e * cos_E;
			
			// Guard against division by zero (singularity at parabolic/hyperbolic orbits)
			if fp.abs() < 1e-15 {
				return E;
			}
			
			// Standard Newton-Raphson update: E = E - f(E)/f'(E)
			E = E - f / fp;
			
			// Fixed-point convergence check: identical to previous iteration (machine precision)
			if (E - E_prev1).abs() <= EPS {
				return E;
			}
			
			// Detect oscillation: if E equals value from 2 iterations ago, we're stuck in a cycle
			if iteration > 0 && (E - E_prev2).abs() <= EPS {
				// Return average of oscillating values
				return 0.5 * (E + E_prev1);
			}
		}
		
		E
	}

	pub fn propagate_to_E_v_r(&self, t: f64, mu: f64, prev_E: Option<f64>) -> (f64, f64, f64) {
		let a = self.a; // km
		let e = self.ecc;
		let n = (mu / (a * a * a)).sqrt(); // rad/s
		let dt = t - self.t0;
		let M = self.m0 + n * dt;
		let E = KeplerianElts::solve_eccentric_anomaly(M, e, prev_E);
		let v = 2.0 * (( (1.0 + e) * (E/2.0).sin() ).sqrt()).atan2(((1.0 - e) * (E/2.0).cos()).sqrt());
		let r = a * (1.0 - e * E.cos());
		(E, v, r)
	}

	pub fn state_from_eccentric_anomaly(&self, E: f64, mu: f64) -> StateVector {
		let a = self.a;
		let e = self.ecc;
		let (sinE, cosE) = E.sin_cos();
		let sqrt_1_e2 = (1.0 - e * e).sqrt();

		let x_pf = a * (cosE - e);
		let y_pf = a * sqrt_1_e2 * sinE;
		let r_km = (x_pf * x_pf + y_pf * y_pf).sqrt();

		let coef = (mu * a).sqrt() / r_km;
		let vx_pf = -coef * sinE;
		let vy_pf = coef * sqrt_1_e2 * cosE;

		let pos_pf = cgmath::Vector3::new(x_pf, y_pf, 0.0);
		let vel_pf = cgmath::Vector3::new(vx_pf, vy_pf, 0.0);

		let rot = self.rotation;
		let pos_inert = rot.rotate_vector(pos_pf);
		let vel_inert = rot.rotate_vector(vel_pf);

		StateVector {
			position: pos_inert,
			velocity: vel_inert,
		}
	}
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProximityReference {
    HillSphere,
    Soi,
}

impl Default for ProximityReference {
    fn default() -> Self {
        ProximityReference::HillSphere
    }
}

#[derive(Clone, Debug)]
pub struct ClosestApproach {
	pub event_id: u32,
	pub body_id: i32,
	pub et: f64,
	pub distance: f64,
	pub position: Vec3d,
	pub rel_velocity: f64,
	pub high_proximity_entry_et: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct CloseApproachEvent {
	pub id: u32,
	pub body_id: i32,
	pub start_idx: usize,
	pub end_idx: usize,
}

pub struct Gaussfg {
	pub f_hat: f64,
	pub f_dot: f64,
	pub g: f64,
	pub g_hat_dot: f64,
}

#[derive(Default)]
pub struct Dop853SharedCache {
	cached_body_states: Vec<StateVector>,
	cached_body_mus: Vec<f64>,
	cur_inf_bodies_ids: Vec<i32>,
	coord_system: super::simulation::CoordSystem,
}

pub struct Dop853BodyAccel {
	shared_cache: Arc<RwLock<Dop853SharedCache>>,
	naif_celestial_objects: Arc<HashMap<i32, celestial::NaifCelestial>>,
}

impl Dop853BodyAccel {
	fn from_shared_cache(
		shared_cache: Arc<RwLock<Dop853SharedCache>>,
		naif_celestial_objects: Arc<HashMap<i32, celestial::NaifCelestial>>,
	) -> Self {
		Self {
			shared_cache,
			naif_celestial_objects,
		}
	}

	fn compute_total_acc(
		&self,
		state_at_epoch: StateAtEpoch,
		only_planetary_perturbations: bool,
	) -> Vec3d {
		if let Ok(shared_cache) = self.shared_cache.read() {
			Integrator::compute_total_acc_from_cached_bodies(
				&shared_cache.cached_body_states,
				&shared_cache.cached_body_mus,
				state_at_epoch,
				only_planetary_perturbations,
			)
		} else {
			Vec3d::new(0.0, 0.0, 0.0)
		}
	}

	fn update_cached_positions(&self, time: f64) {
		if let Ok(mut shared_cache) = self.shared_cache.write() {
			shared_cache.cached_body_states.clear();
			for &id in shared_cache.cur_inf_bodies_ids.clone().iter() {
				if let Some(body) = self.naif_celestial_objects.get(&id) {
					if let Some(body_state) = body.glob_state_at_time(time, "ECLIPJ2000", &shared_cache.coord_system) {
						shared_cache.cached_body_states.push(body_state);
					} else {
						shared_cache.cached_body_states.push(StateVector::default());
					}
				} else {
					shared_cache.cached_body_states.push(StateVector::default());
				}
			}
		}
	}
}

impl System<f64, SVector<f64, 6>> for Dop853BodyAccel {
	fn system(&self, t: f64, y: &SVector<f64, 6>, dy: &mut SVector<f64, 6>) {
		// dr/dt = v
		dy[0] = y[3];
		dy[1] = y[4];
		dy[2] = y[5];

		self.update_cached_positions(t);
		let acc = self.compute_total_acc(StateAtEpoch::from_svector_and_epoch(*y, t), false);

		// dv/dt = a
		dy[3] = acc[0];
		dy[4] = acc[1];
		dy[5] = acc[2];

	}

	fn solout(&mut self, _x: f64, _y: &SVector<f64, 6>, _dy: &SVector<f64, 6>) -> bool {
		true 
		// Because we use system in the step func, so after 1 step we get the solution.
		// Perhaps, needs to be refactored, could run this in a proper loop but adds complexity with calling updaters, 
		// cache syncing etc. And in this case we would solout if the last proximity flag is 0, but before was 1
	}
}

// TODO: after integrating, count len of proximity flags. Or limit start_time - end_time to something reasonable.
// OR if len >= u32::MAX - return err. All these cases mean that the trajectory is too long + currently trajectory pipeline
// draws all points in 1 draw call (will split if needed, however i do not expect users to integrate trajectories 
// for millions of years, so should be fine for now).

// RULE: ***_step funcs take state and dt, return new state after dt. they do not update cur_time or states vec. this is done in the main loop.
			
pub struct Integrator {
    pub sb_id: i32,
	/// et seconds
    pub start_time: f64, 
	/// et seconds
    pub end_time: f64, 
    pub initial_glob_state: StateVector, 
    cur_time: f64,
    dt: f64,
    tol: f64,
    min_scale: f64,
    max_scale: f64,
    coord_system: super::simulation::CoordSystem,
    proximity_reference: ProximityReference,
    proximity_multiplier: f64,
    use_adaptive_dt: bool,
    naif_celestial_objects: Arc<HashMap<i32, celestial::NaifCelestial>>,
	/// same len as proximity_flags
    states: Vec<StateAtEpoch>, 
    proximity_flags: Vec<u32>,
    cur_inf_bodies_ids: Vec<i32>,
	/// sun, planets, and moon (if NEO)
    permanent_body_ids: Vec<i32>, 
	/// indexed like cur_inf_bodies_ids
    cached_body_states: Vec<StateVector>, 
	/// indexed like cur_inf_bodies_ids
    cached_body_mus: Vec<f64>,
	/// indexed like permanent bodies (planets)
    cached_hill_spheres: Vec<Option<f64>>, 
	cur_inf_bodies_changed: bool,

	closest_approach_events: Vec<CloseApproachEvent>,
	closest_approaches_data: Vec<ClosestApproach>,

	whfast_dt: f64,

	dop853_shared_cache: Arc<RwLock<Dop853SharedCache>>,
	dop853: Option<Dop853<f64, SVector<f64, 6>, Dop853BodyAccel>>,
	last_dop853_dt: f64,
}

impl Integrator {

    pub fn new(
        sb_id: i32,
        start_time: f64,
        end_time: f64,
        initial_glob_state: StateVector, // x,y,z,vx,vy,vz
        dt: f64,
        naif_celestial_objects: Arc<HashMap<i32, celestial::NaifCelestial>>,
        proximity_reference: ProximityReference,
        proximity_multiplier: f64,
        coord_system: super::simulation::CoordSystem
    ) -> Self {

        let mut cur_inf_bodies_ids: Vec<i32> = spice_ker::STANDARD_PLANET_IDS.iter().copied().collect();
        cur_inf_bodies_ids.push(10); // Sun, its index is 9 because ^
        
        // Moon if this is a NEO
        let is_neo = sbdb::is_neo(sb_id).unwrap_or(false);
        if is_neo {
            cur_inf_bodies_ids.push(301); // Moon
        }
        
        let permanent_body_ids = cur_inf_bodies_ids.clone();
        let cached_body_states = vec![StateVector::default(); cur_inf_bodies_ids.len()];
        let cached_body_mus = vec![0.0; cur_inf_bodies_ids.len()];
        let cached_hill_spheres = vec![None; spice_ker::STANDARD_PLANET_IDS.len()];
		let dop853_shared_cache = Arc::new(RwLock::new(Dop853SharedCache::default()));
        
        Integrator {
            sb_id,
            start_time,
            end_time,
            cur_time: start_time,
            initial_glob_state,
            dt,
            tol: 1.0e-8,
            min_scale: 0.3333333333,
            max_scale: 6.0,
            coord_system,
            proximity_reference,
            proximity_multiplier,
            use_adaptive_dt: true,
            naif_celestial_objects,
            states: vec![StateAtEpoch::from_state_and_epoch(initial_glob_state, start_time)],
            cur_inf_bodies_ids,
            permanent_body_ids,
            cached_body_states,
            cached_body_mus,
            cached_hill_spheres,
            proximity_flags: Vec::new(),
			closest_approach_events: Vec::new(),
			closest_approaches_data: Vec::new(),
			whfast_dt: 86400.0,   // 1 day in seconds, could be user-defined or adaptive based on proximity
			cur_inf_bodies_changed: false,
			dop853_shared_cache,
			dop853: None,
			last_dop853_dt: 60.0,
        }
    }

	fn update_cached_positions(&mut self, time: f64) {
		self.cached_body_states.clear();
		for &id in self.cur_inf_bodies_ids.iter() {
			if let Some(body) = self.naif_celestial_objects.get(&id) {
				if let Some(body_state) = body.glob_state_at_time(time, "ECLIPJ2000", &self.coord_system) {
					self.cached_body_states.push(body_state);
				} else {
					self.cached_body_states.push(StateVector::default());
				}
			} else {
				self.cached_body_states.push(StateVector::default());
			}
		}
	}

	fn compute_total_acc(
		&self,
		state_at_epoch: StateAtEpoch, 
		only_planetary_perturbations: bool 
	) -> Vec3d {
		Self::compute_total_acc_from_cached_bodies(
			&self.cached_body_states,
			&self.cached_body_mus,
			state_at_epoch,
			only_planetary_perturbations,
		)
	}

	fn compute_total_acc_from_cached_bodies(
		cached_body_states: &[StateVector],
		cached_body_mus: &[f64],
		state_at_epoch: StateAtEpoch,
		only_planetary_perturbations: bool,
	) -> Vec3d {
		let mut total_acc = Vec3d::new(0.0, 0.0, 0.0);
		for idx in 0..cached_body_states.len() {
			let mu = cached_body_mus[idx];
			if mu <= 0.0 {
				continue;
			}

			if only_planetary_perturbations && idx == 9 {
				continue;
			}

			let body_pos = &cached_body_states[idx].position;
			let r_vec = state_at_epoch.state.position - body_pos;

			let grav_acc = Self::grav_acceleration(mu, r_vec);

			total_acc[0] += grav_acc[0];
			total_acc[1] += grav_acc[1];
			total_acc[2] += grav_acc[2];
		}
		total_acc
	}

	fn sync_dop853_shared_cache(&self) {
		let mut shared_cache = self.dop853_shared_cache.write().unwrap();
		shared_cache.cached_body_states = self.cached_body_states.clone();
		shared_cache.cached_body_mus = self.cached_body_mus.clone();
		shared_cache.cur_inf_bodies_ids = self.cur_inf_bodies_ids.clone();
		shared_cache.coord_system = self.coord_system;
	}

    fn step_state_verlet(&mut self, state_at_epoch: StateAtEpoch, dt: f64) -> StateAtEpoch {
		self.update_cached_positions(state_at_epoch.et);
		let acc0 = self.compute_total_acc(state_at_epoch, false);

		// Midpoint velocity
		let v_mid: Vec3d = state_at_epoch.state.velocity + acc0 * (dt * 0.5);

		// New position after full step
		let x_new: Vec3d = state_at_epoch.state.position + v_mid * dt;


		// Re-evaluate perturber positions at the end of the trial step for acc1.
		self.update_cached_positions(state_at_epoch.et + dt);

		// re-evaluate grav_acceleration at the new position (using cached body positions)
		let new_state_for_acc = StateAtEpoch::from_state_and_epoch(
			StateVector {
				position: x_new,
				velocity: v_mid,
			},
			state_at_epoch.et + dt,
		);
		let acc1 = self.compute_total_acc(new_state_for_acc, false);

		// New velocity using grav_acceleration at new position
		let v_new: Vec3d = v_mid + acc1 * (dt * 0.5);

		StateAtEpoch::from_state_and_epoch(
			StateVector {
				position: x_new,
				velocity: v_new,
			},
			state_at_epoch.et + dt,
		)

	}

	pub fn dop853_step(
		&mut self, 
		state_at_epoch: StateAtEpoch, 
		dt: f64,
	) -> StateAtEpoch {
		if dt <= 0.0 {
			return state_at_epoch;
		}

		if self.dop853.is_none() {
			let system = Dop853BodyAccel::from_shared_cache(
				Arc::clone(&self.dop853_shared_cache),
				Arc::clone(&self.naif_celestial_objects),
			);
			let initial_state = SVector::<f64, 6>::new(
				state_at_epoch.state.position.x,
				state_at_epoch.state.position.y,
				state_at_epoch.state.position.z,
				state_at_epoch.state.velocity.x,
				state_at_epoch.state.velocity.y,
				state_at_epoch.state.velocity.z,
			);
			let initial_step_guess = if self.last_dop853_dt > 0.0 {
				self.last_dop853_dt.min(dt.abs())
			} else {
				0.0
			};

			self.dop853 = Some(Dop853::from_param(
				system,
				state_at_epoch.et,
				self.end_time,
				dt.abs(),
				initial_state,
				self.tol,
				self.tol,
				0.9,
				0.0,
				self.min_scale,
				self.max_scale,
				self.end_time - state_at_epoch.et,
				initial_step_guess,
				100000,
				1000,
				OutputType::Sparse,
			));
		}

		let Some(solver) = self.dop853.as_mut() else {
			return state_at_epoch;
		};

		match solver.integrate() {
			Ok(_) => {
				let output_times = solver.x_out();
				let output_states = solver.y_out();

				if output_times.len() >= 2 && output_states.len() >= 2 {
					// `solout()` stops after one accepted DOP853 step, so the last two output times
					// bracket exactly the step that was just accepted by the solver.
					let previous_time = output_times[output_times.len() - 2];
					let current_time = output_times[output_times.len() - 1];
					let accepted_dt = (current_time - previous_time).abs();
					if accepted_dt.is_finite() && accepted_dt > 0.0 {
						self.last_dop853_dt = accepted_dt;
					}

					let state_last = &output_states[output_states.len() - 1];
					StateAtEpoch::from_state_and_epoch(
						StateVector {
							position: Vector3::new(state_last[0], state_last[1], state_last[2]),
							velocity: Vector3::new(state_last[3], state_last[4], state_last[5]),
						},
						current_time,
					)
				} else {
					state_at_epoch
				}
			}
			Err(_) => {
				self.dop853 = None;
				state_at_epoch
			}
		}
	}

	/// When far (GREATLY simplified because only 1 small body)
	/// Adapted from "WHFast: A fast and unbiased implementation of 
	/// a symplectic Wisdom-Holman integrator for long term gravitational simulations",
	/// Hanno Rein, Daniel Tamayo, 2015
	pub fn whfast_step(
		&mut self, 
		state_at_epoch: StateAtEpoch, 
		dt: f64,
	) -> StateAtEpoch {
		// drift dt/2 -> kick dt -> drift dt/2

		let mut res_state = state_at_epoch;

		// MARK: barycentercentric is not implemented yet. 

		let sun_mu = self.cached_body_mus[9]; // we have written it in new()
		if !sun_mu.is_finite() || sun_mu <= 0.0 {
			warn!("Sun GM is missing or zero, falling back to Verlet for this step");
			return self.step_state_verlet(state_at_epoch, dt);
		}
		let dt_2 = 0.5 * dt;

		let drift1_state = Self::solve_kepler_state(state_at_epoch.state, sun_mu, dt_2);
		res_state.state.position = drift1_state.position;
		res_state.state.velocity = drift1_state.velocity;
		res_state.et = state_at_epoch.et + dt_2;

		self.update_cached_positions(state_at_epoch.et + dt_2);

		let kick_acc = self.compute_total_acc(res_state, true);

		res_state.state.velocity += kick_acc * dt;

		let drift2_state = Self::solve_kepler_state(res_state.state, sun_mu, dt_2);
		res_state.state.position = drift2_state.position;
		res_state.state.velocity = drift2_state.velocity;

		res_state.et = state_at_epoch.et + dt;

		res_state

	}

	pub fn solve_kepler_state(state0: StateVector, sun_mu: f64, dt: f64) -> StateVector {
		let r0 = state0.position;
		let v0 = state0.velocity;

		let r0_mag = r0.magnitude();
		let v0_mag = v0.magnitude();

		let beta = Self::beta(r0_mag, v0_mag, sun_mu);
		let eta0 = Self::eta0(r0, v0);
		let zeta0 = Self::zeta0(sun_mu, beta, r0_mag);
		let zeta = Self::zeta(zeta0, dt, r0_mag);

		let X = Self::solve_kepler_X(r0_mag, dt, zeta, zeta0, eta0, beta);

		let G1 = Self::stumpff_G(1, beta, X);
		let G2 = Self::stumpff_G(2, beta, X);
		let G3 = Self::stumpff_G(3, beta, X);

		let gauss_fg = Self::Gauss_f_g(sun_mu, r0_mag, dt, eta0, zeta0, G1, G2, G3);

		let r = (gauss_fg.f_hat * r0 + gauss_fg.g * v0) + r0;
		let v = (gauss_fg.f_dot * r0 + gauss_fg.g_hat_dot * v0) + v0;
		StateVector {
			position: r,
			velocity: v,
		}
	}

	/// mu = G * (m_central + m_orbiting) (to avoid confusion)
	/// hat - small deviation of rectilinear motion
	pub fn Gauss_f_g(
		mu: f64, 
		r0: f64, dt: f64, 
		eta0: f64, zeta0: f64,
		G1: f64, G2: f64, G3: f64
	) -> Gaussfg {

		let r = r0 + eta0 * G1 + zeta0 * G2;

		let f_hat = -mu * (G2/r0);
		let f_dot = -(mu * G1) / (r0 * r);
		
		let g = dt - mu * G3;
		let g_hat_dot = -(mu * G2) / r;

		Gaussfg {
			f_hat,
			f_dot,
			g,
			g_hat_dot,
		}
	}

	pub fn solve_kepler_X(r0: f64, dt: f64, zeta: f64, zeta0: f64, eta0: f64, beta: f64) -> f64 {
		let mut X = Self::kepler_init_guess(r0, dt, zeta);
		let mut X_prev1 = 0.0;
		let mut X_prev2: f64;
		for _ in 0..50 {
			X_prev2 = X_prev1;
			X_prev1 = X;
			X = Self::kepler_step_X(X, eta0, zeta0, r0, dt, beta);
			if (X - X_prev1).abs() <= EPS || (X - X_prev2).abs() <= EPS { // check for convergence or oscillation
				break;
			}
		}
		X
	}

	/// universal anomaly formulation for all conic sections. 
	/// In papers eta is similar to n (symbol) and zeta is something other
	pub fn kepler_step_X(X: f64, eta0: f64, zeta0: f64, r0: f64, dt: f64, beta: f64) -> f64 {
		let G1 = Self::stumpff_G(1, beta, X);
		let G2 = Self::stumpff_G(2, beta, X);
		let G3 = Self::stumpff_G(3, beta, X);
		let numerator = X * (eta0 * G1 + zeta0 * G2) - eta0 * G2 - zeta0 * G3 + dt;
		let denominator = r0 + eta0 * G1 + zeta0 * G2;
		numerator / denominator
	}

	pub fn kepler_init_guess(r0: f64, dt: f64, zeta: f64) -> f64 {
		(dt/r0) * (1.0 - 0.5*zeta)
	}

	/// mu = G * (m_central + m_orbiting) (to avoid confusion)
	pub fn beta(r0: f64, v0: f64, mu: f64) -> f64 { 
		(2.0 * mu) / r0 - (v0 * v0)
	}

	pub fn eta0(r0_vec: Vector3<f64>, v0_vec: Vector3<f64>) -> f64 {
		r0_vec.dot(v0_vec)
	}
	
	/// mu = G * (m_central + m_orbiting) (to avoid confusion)
	pub fn zeta0(mu: f64, beta: f64, r0: f64) -> f64 {
		mu - beta * r0
	}

	pub fn eta(eta0: f64, dt: f64, r0: f64) -> f64 {
		(eta0 * dt) / (r0 * r0)
	}

	pub fn zeta(zeta0: f64, dt: f64, r0: f64) -> f64 {
		(zeta0 * dt * dt) / (r0 * r0 * r0)
	} 

	pub fn stumpff_G(n: u32, beta: f64, X: f64) -> f64 {
		X.pow(n as f64) * Self::stumpff_c(beta * X * X, n)
	}

	/// if z > 0 - ellipse, z = 0 - parabola, z < 0 - hyperbola
	pub fn stumpff_c(z: f64, n_c: u32) -> f64 { 
		let mut n = 0;
		let mut c: [f64; 6] = [0.0; 6];
		let mut z = z;
		while z > 0.1 {
			z *= 0.25;	
			n += 1;
		}
		c[4] = FACTORIAL_INV[4] - z/720.0; // 1/4! - z/6!
		c[5] = FACTORIAL_INV[5] - z/5040.0; // 1/5! - z/7!

		let z_ = -z;
		let mut p = z_;
		let mut k = 8; 
		let mut series_iters = 0usize;

		loop {
			let c4_prev = c[4];
			p = p * z_;
			c[4] = c[4] + p * FACTORIAL_INV[k as usize];
			k += 1;
			c[5] = c[5] + p * FACTORIAL_INV[k as usize];
			k += 1;
			series_iters += 1;
			if (c4_prev - c[4]).abs() <= EPS || series_iters >= STUMPF_C_MAX_ITER {
				break;
			}
		}

		c[3] = 1.0/6.0 - z * c[5];
		c[2] = 0.5 - z * c[4];
		c[1] = 1.0 - z * c[3];

		let mut scaling_iters = 0usize;
		while n > 0 {
			z *= 4.0;
			c[5] = 1.0/16.0 * (c[5] + c[4] + c[3] + c[2]);
			c[4] = 1.0/8.0 * c[3] * (1.0 + c[1]);
			c[3] = 1.0/6.0 - z * c[5];
			c[2] = 0.5 - z * c[4];
			c[1] = 1.0 - z * c[3];
			n -= 1;
			scaling_iters += 1;
			if scaling_iters >= STUMPF_C_MAX_ITER {
				break;
			}
		}
		c[0] = 1.0 - z * c[2];


		c[n_c as usize]
	}

	//////////////////////////////////////////////////////

	fn update_current_inf_bodies(&mut self) -> u32 {
		let mut is_close_any: u32 = 0u32;
		let mut should_update_constituents = false;
		let prev_inf_bodies_ids = self.cur_inf_bodies_ids.clone();
		let current_state_idx = self.states.len().saturating_sub(1);
		let mut close_body_ids = Vec::new();
		
		// Check if we're close to any planet; if so, update every step
		for (body_idx, &id) in spice_ker::STANDARD_PLANET_IDS.iter().enumerate() {
			if body_idx < self.cached_body_states.len() {
				let body_state = self.cached_body_states[body_idx];
				let dist = self.cur_dist_to_body(body_state.position);
				
				if let Some(hill_sphere_radius) = self.cached_hill_spheres.get(body_idx).and_then(|&hs| hs) {
					if dist <= hill_sphere_radius * 3.0 {
						should_update_constituents = true;
					}
				}
				
				if let Some(body) = self.naif_celestial_objects.get(&id) {
					if let Some(proximity_threshold) = self.body_proximity_threshold(body) {
						if dist <= proximity_threshold {
							is_close_any = 1u32;
							close_body_ids.push(id);
						}
					}
				}
			}
		}

		for &body_id in &close_body_ids {
			// Checking if we already have an event (searching for the last event with this body_id, 
			// and if its end_idx is current or next index, then update end_idx instead of creating a new event).
			// BUT if the last event with this body_id ended more than 1 step ago, that basically means that
			// we exited and re-entered the proximity zone, so we should create a new event instead of updating the old one.
			if let Some(event) = self.closest_approach_events.iter_mut().rev().find(|event| event.body_id == body_id){
				let diff = current_state_idx as isize - event.end_idx as isize;
				if diff == 1 {
					event.end_idx = current_state_idx;
					continue;
				} else if diff > 1 {
					let event_id = self.closest_approach_events.len() as u32;
					self.closest_approach_events.push(CloseApproachEvent {
						id: event_id,
						body_id,
						start_idx: current_state_idx,
						end_idx: current_state_idx,
					});
					continue;
				} else {
					// dk how this can happen
					continue;
				}
			}

			let event_id = self.closest_approach_events.len() as u32;
			self.closest_approach_events.push(CloseApproachEvent {
				id: event_id,
				body_id,
				start_idx: current_state_idx,
				end_idx: current_state_idx,
			});
		}
		
		if should_update_constituents {
			
			self.cur_inf_bodies_ids.retain(|id| self.permanent_body_ids.contains(id));
			
			// Add constituents for bodies within 3× Hill sphere
			for (body_idx, &id) in spice_ker::STANDARD_PLANET_IDS.iter().enumerate() {
				if body_idx < self.cached_body_states.len() {
					let body_state = self.cached_body_states[body_idx];
					let dist = self.cur_dist_to_body(body_state.position);
					
					if let Some(hill_sphere_radius) = self.cached_hill_spheres.get(body_idx).and_then(|&hs| hs) {
						if dist <= hill_sphere_radius * 3.0 {
							if let Some(body) = self.naif_celestial_objects.get(&id) {
								for c in body.constituents_ids.iter() {
									if !self.cur_inf_bodies_ids.contains(c) {
										self.cur_inf_bodies_ids.push(*c);
									}
								}
							}
						}
					}
				}
			}
		} 

		self.cur_inf_bodies_changed = self.cur_inf_bodies_ids != prev_inf_bodies_ids;
		
		is_close_any
	}

    fn body_proximity_threshold(&self, body: &celestial::NaifCelestial) -> Option<f64> {
        let base_radius = match self.proximity_reference {
            ProximityReference::HillSphere => body.hill_sphere_radius(),
            ProximityReference::Soi => body.soi_radius(),
        };
        base_radius.map(|r| r * self.proximity_multiplier)
    }

	fn cache_body_parameters(&mut self) {
		self.cached_body_mus.clear();
		for &id in self.cur_inf_bodies_ids.iter() {
			if let Some(body) = self.naif_celestial_objects.get(&id) {
				let mu = body.physical_params.as_ref().map(|p| p.mu).unwrap_or(0.0);
				self.cached_body_mus.push(mu);
			} else {
				self.cached_body_mus.push(0.0);
			}
		}

		self.cached_hill_spheres.clear();
		for &id in spice_ker::STANDARD_PLANET_IDS.iter() {
			if let Some(body) = self.naif_celestial_objects.get(&id) {
				self.cached_hill_spheres.push(body.hill_sphere_radius());
			} else {
				self.cached_hill_spheres.push(None);
			}
		}
	}

    pub fn run_with_progress(
		&mut self,
		progress_tx: Option<&mpsc::Sender<f32>>,
	) {
		self.closest_approach_events.clear();
		self.closest_approaches_data.clear();
		self.proximity_flags.clear();
		self.cur_inf_bodies_changed = false;
		
		self.cache_body_parameters();
		
		self.update_cached_positions(self.cur_time);
		let initial_flag = self.update_current_inf_bodies();
		if self.cur_inf_bodies_changed {
			self.cache_body_parameters();
			self.update_cached_positions(self.cur_time);
			self.cur_inf_bodies_changed = false;
		}
		self.sync_dop853_shared_cache();
		self.proximity_flags.push(initial_flag); // proximity_flags is guaranteed to have at least 1 element.

		while self.cur_time < self.end_time {
			let mut dt_trial = self.dt;
			let remaining = self.end_time - self.cur_time;
			if dt_trial > remaining {
				dt_trial = remaining;
			}

			let last_state = *self.states.last().unwrap_or(&StateAtEpoch::from_state_and_epoch(self.initial_glob_state, self.start_time));

			if self.proximity_flags.last().copied().unwrap_or(0u32) == 0u32 {
				self.dop853 = None;
				let wh_dt = self.whfast_dt.min(remaining);
				if wh_dt <= 0.0 {
					break;
				}
				let new_state =self.whfast_step(
					last_state, 
					wh_dt, 
				);
				self.states.push(new_state);
				self.cur_time += wh_dt;
				
				self.update_cached_positions(self.cur_time);
				let flag = self.update_current_inf_bodies();
				if self.cur_inf_bodies_changed {
					self.cache_body_parameters();
					self.update_cached_positions(self.cur_time);
					self.cur_inf_bodies_changed = false;
				}
				self.sync_dop853_shared_cache();
				self.proximity_flags.push(flag);

				if let Some(tx) = progress_tx {
					let span = self.end_time - self.start_time;
					if span > 0.0 {
						let frac = ((self.cur_time - self.start_time) / span)
							.clamp(0.0, 1.0) as f32;
						let _ = tx.send(frac);
					}
				}
				continue;
			}

			if self.use_adaptive_dt {
				// Near a body we switch fully to DOP853; the old Verlet trial/error loop is no longer used here.
				self.sync_dop853_shared_cache();
				let next_state = self.dop853_step(
					last_state,
					dt_trial,
				);
				if next_state.et <= last_state.et {
					return;
				}

				// DOP853 chooses the accepted internal step size; keep that as the next guess.
				self.states.push(next_state);
				self.cur_time = next_state.et;
				self.dt = self.last_dop853_dt;
				
				self.update_cached_positions(self.cur_time);
				let flag = self.update_current_inf_bodies();
				if self.cur_inf_bodies_changed {
					self.cache_body_parameters();
					self.update_cached_positions(self.cur_time);
					self.cur_inf_bodies_changed = false;
				}
				self.sync_dop853_shared_cache();
				self.proximity_flags.push(flag);

				if let Some(tx) = progress_tx {
					let span = self.end_time - self.start_time;
					if span > 0.0 {
						let frac = ((self.cur_time - self.start_time) / span)
							.clamp(0.0, 1.0) as f32;
						let _ = tx.send(frac);
					}
				}
			} else {
				self.dop853 = None;
				let next_state = self.step_state_verlet(
					last_state,
					dt_trial,
				);
				self.states.push(next_state);
				self.cur_time += dt_trial;
				
				self.update_cached_positions(self.cur_time);
				let flag = self.update_current_inf_bodies();
				if self.cur_inf_bodies_changed {
					self.cache_body_parameters();
					self.update_cached_positions(self.cur_time);
					self.cur_inf_bodies_changed = false;
				}
				self.sync_dop853_shared_cache();
				self.proximity_flags.push(flag);

				if let Some(tx) = progress_tx {
					let span = self.end_time - self.start_time;
					if span > 0.0 {
						let frac = ((self.cur_time - self.start_time) / span)
							.clamp(0.0, 1.0) as f32;
						let _ = tx.send(frac);
					}
				}
			}
		}

		if self.proximity_flags.iter().any(|&flag| flag == 1u32) {
			self.calculate_closest_approaches();
		}
	}

	pub fn positions(&self) -> Vec<Vec3d> {
		self.states.iter().map(|state| state.state.position).collect()
	}

	pub fn states(&self) -> &[StateAtEpoch] {
		&self.states
	}

	pub fn proximity_flags(&self) -> Vec<u32> {
		self.proximity_flags.clone()
	}

	pub fn proximity_flags_ref(&self) -> &[u32] {
		&self.proximity_flags
	}

    pub fn find_tca<F>(&self, planet_state_at: F, sample_range: Option<(usize, usize)>) -> (StateAtEpoch, f64, f64)
    where
        F: Fn(f64) -> StateVector,
    {
        let len = self.states.len();
        assert!(len >= 2, "trajectory must have at least two states for TCA search"); /////

        let (range_start, range_end) = sample_range
            .map(|(start, end)| (start.min(len - 1), end.min(len - 1)))
            .filter(|(start, end)| start < end)
            .unwrap_or((0, len - 1));

        let mut best_i = range_start;
        let mut min_d = f64::INFINITY;

        for i in range_start..=range_end {
            let et = self.states[i].et;
            let p_pl_state = planet_state_at(et);
            let r_rel = self.states[i].state.position - p_pl_state.position;
            let d = (r_rel[0] * r_rel[0] + r_rel[1] * r_rel[1] + r_rel[2] * r_rel[2]).sqrt();

            if d < min_d {
                min_d = d;
                best_i = i;
            }
        }

        let get_state_at = |t: f64| {
            if t <= self.states[0].et {
                let mut s = self.states[0];
                s.et = t;
                return s;
            }
            if t >= self.states[len - 1].et {
                let mut s = self.states[len - 1];
                s.et = t;
                return s;
            }
            let idx = self.states.partition_point(|s| s.et <= t) - 1;
            let interp = Self::hermite_interpolate(&self.states[idx], &self.states[idx + 1], t);
            StateAtEpoch::from_state_and_epoch(interp, t)
        };

        let rel_dot_at = |idx: usize| {
            let et = self.states[idx].et;
            let p_pl_state = planet_state_at(et);
            let r_rel = self.states[idx].state.position - p_pl_state.position;
            let v_rel = self.states[idx].state.velocity - p_pl_state.velocity;
            r_rel.dot(v_rel)
        };

        let mut left = best_i.saturating_sub(1).max(range_start);
        let mut right = (best_i + 1).min(range_end);

        if best_i > range_start {
            let prev_dot = rel_dot_at(best_i - 1);
            let cur_dot = rel_dot_at(best_i);
            if prev_dot * cur_dot <= 0.0 {
                left = best_i - 1;
                right = best_i;
            }
        }
        if right == best_i && best_i < range_end {
            let cur_dot = rel_dot_at(best_i);
            let next_dot = rel_dot_at(best_i + 1);
            if cur_dot * next_dot <= 0.0 {
                left = best_i;
                right = best_i + 1;
            }
        }
        if left == right {
            if right < range_end {
                right += 1;
            } else if left > range_start {
                left -= 1;
            }
        }

        let (mut t0, mut t1) = (self.states[left].et, self.states[right].et);

        let state0 = get_state_at(t0);
        let p0_state = planet_state_at(t0);
        let r_rel0 = state0.state.position - p0_state.position;
        let v_rel0 = state0.state.velocity - p0_state.velocity;
        let mut f0 = r_rel0.dot(v_rel0);

        const TOL_DT: f64 = 1e-6;
        let mut state_mid: StateAtEpoch;
        let mut d_mid: f64;

        for _ in 0..40 {
            let t_mid = 0.5 * (t0 + t1);
            state_mid = get_state_at(t_mid);
            let p_mid_state = planet_state_at(t_mid);
            let r_rel_mid = state_mid.state.position - p_mid_state.position;
            let v_rel_mid = state_mid.state.velocity - p_mid_state.velocity;
            let f_mid = r_rel_mid.dot(v_rel_mid);
            d_mid = r_rel_mid.magnitude();

            if f_mid.abs() < 1e-12 || (t1 - t0) < TOL_DT {
                let rel_vel_mag = v_rel_mid.magnitude();
                return (state_mid, d_mid, rel_vel_mag);
            }

            if f0 * f_mid < 0.0 {
                t1 = t_mid;
            } else {
                t0 = t_mid;
                f0 = f_mid;
            }
        }

        let final_t = 0.5 * (t0 + t1);
        let final_state = get_state_at(final_t);
        let p_final_state = planet_state_at(final_t);
        let r_rel_final = final_state.state.position - p_final_state.position;
        let v_rel_final = final_state.state.velocity - p_final_state.velocity;
        let final_d = r_rel_final.magnitude();
        let rel_vel_mag = v_rel_final.magnitude();
        (final_state, final_d, rel_vel_mag)
    }

    pub fn cur_dist_to_body(&self, body_pos: Vec3d) -> f64 {
		let default_state = StateAtEpoch::from_state_and_epoch(self.initial_glob_state, self.start_time);
        let last_state = self.states.last().unwrap_or(&default_state);
		
       last_state.state.position.distance(body_pos)
    }

	pub fn state_rel_to_ssb_at_et(state_rel_to_sun: StateVector, et: f64) -> StateVector {
		let ssb_state_rel_to_sun = utils::celestial_state_et(et, 0, 10, "ECLIPJ2000").ok().unwrap_or(StateVector::default());

		state_rel_to_sun - ssb_state_rel_to_sun
	}

	pub fn grav_acceleration(mu: f64, r: Vec3d) -> Vec3d {
		let r2 = r.magnitude2();
		-mu * r / (r2 * r2.sqrt())
	}

	// TODO all of this
	/// Acceleration due to relativistic correction (Einstein–Infeld–Hoffmann equations)
	pub fn rel_correction_acceleration() -> Vec3d {
		todo!()
	}	

	pub fn spherical_harmonics_acceleration() -> Vec3d {
		todo!()
	}

	/// Acceleration due to solar radiation pressure
	pub fn solar_radiation_pressure_acceleration() -> Vec3d {
		todo!()
	}

	pub fn hermite_interpolate(state1: &StateAtEpoch, state2: &StateAtEpoch, et: f64) -> StateVector {
		let t0 = state1.et;
		let t1 = state2.et;
		let h = t1 - t0;
		let tau = (et - t0) / h;

		let tau2 = tau * tau;
		let tau3 = tau2 * tau;

		// Hermite basis functions (position interpolation)
		let h00 = 2.0 * tau3 - 3.0 * tau2 + 1.0; 
		let h10 = tau3 - 2.0 * tau2 + tau;      
		let h01 = -2.0 * tau3 + 3.0 * tau2;       
		let h11 = tau3 - tau2;                   

		// Derivatives of Hermite basis functions (velocity interpolation)
		// d/dtau of H00: 6*tau2 - 6*tau
		// d/dtau of H10: 3*tau2 - 4*tau + 1
		// d/dtau of H01: -6*tau2 + 6*tau
		// d/dtau of H11: 3*tau2 - 2*tau
		let dh00 = 6.0 * tau2 - 6.0 * tau;
		let dh10 = 3.0 * tau2 - 4.0 * tau + 1.0;
		let dh01 = -6.0 * tau2 + 6.0 * tau;
		let dh11 = 3.0 * tau2 - 2.0 * tau;

		let mut result = StateVector::default();

		// Position interpolation: p(t) = H00*p0 + H10*(h*v0) + H01*p1 + H11*(h*v1)
		result.position = h00 * state1.state.position 
						+ h10 * (h * state1.state.velocity)
						+ h01 * state2.state.position 
						+ h11 * (h * state2.state.velocity);

		// Velocity interpolation: v(t) = (1/h) * [H00'*p0 + H10'*(h*v0) + H01'*p1 + H11'*(h*v1)]
		// Simplified: v(t) = (1/h) * [H00'*p0 + H10'*h*v0 + H01'*p1 + H11'*h*v1]
		// Simply the derivative of the position interpolation formula with respect to time.
		result.velocity = (dh00 * state1.state.position 
						+ dh10 * (h * state1.state.velocity)
						+ dh01 * state2.state.position 
						+ dh11 * (h * state2.state.velocity)) / h;

		result
	}

	/// Interpolated
	pub fn state_at(&self, t: f64) -> StateVector { 
	
		if t <= self.start_time {return StateVector::from(self.initial_glob_state); }
		
		if t >= self.states.last().unwrap_or(&StateAtEpoch::default()).et { 
			return StateVector::from(self.states.last().unwrap_or(&StateAtEpoch::default()).state);
		}

		// find first point where states[i][6] > t
		let i = self.states.partition_point(|s| s.et <= t) - 1;
		Self::hermite_interpolate(&self.states[i], &self.states[i+1], t)
	}

	fn calculate_closest_approaches(&mut self) {
		self.closest_approaches_data.clear();

		for event in &self.closest_approach_events {
			let body_id = event.body_id;
			if self.naif_celestial_objects.contains_key(&body_id) {
				let central = match self.coord_system {
					simulation::CoordSystem::BodyCentric => 10,
					simulation::CoordSystem::BarocenterCentric => 0,
				};

				let planet_state_at = |et: f64| {
					if let Ok(state_vector) = utils::celestial_state_et(et, body_id, central, "ECLIPJ2000") {
						state_vector
					} else {
						StateVector::default()
					}
				};

				let proximity_range = Some((event.start_idx, event.end_idx));
				let (tca_state, distance, rel_velocity) = self.find_tca(&planet_state_at, proximity_range);
				let position = tca_state.state.position;
				let high_proximity_entry_et = proximity_range.map(|(start_idx, _)| self.states[start_idx].et);
				self.closest_approaches_data.push(ClosestApproach {
					event_id: event.id,
					body_id,
					et: tca_state.et,
					distance,
					position,
					rel_velocity,
					high_proximity_entry_et,
				});
			}
		}
	}

	pub fn closest_approaches(&self) -> Vec<ClosestApproach> {
		self.closest_approaches_data.clone()
	}

	pub fn closest_approaches_ref(&self) -> &[ClosestApproach] {
		&self.closest_approaches_data
	}

	pub fn closest_approach_points(&self) -> Vec<Vec3d> {
		self.closest_approaches_data.iter().map(|ca| ca.position).collect()
	}

	pub fn closest_approach_et(&self, planet_id: i32) -> Option<f64> {
		self.closest_approaches_data.iter().find(|ca| ca.body_id == planet_id).map(|ca| ca.et)
	}

	pub fn closest_approach_et_for_event(&self, event_id: u32) -> Option<f64> {
		self.closest_approaches_data.iter().find(|ca| ca.event_id == event_id).map(|ca| ca.et)
	}

	pub fn planet_id_in_closest_approaches(&self, planet_id: i32) -> bool {
		self.closest_approaches_data.iter().any(|ca| ca.body_id == planet_id)
	}

	pub fn planet_id_index_in_closest_approaches(&self, planet_id: i32) -> Option<usize> {
		self.closest_approaches_data.iter().position(|ca| ca.body_id == planet_id)
	}

	/// Returns the start and end indices of the proximity segment for the given event id.
	/// The returned range is inclusive on both ends.
	pub fn start_end_proximity_for_event(&self, event_id: u32) -> Option<(usize, usize)> {
		self.closest_approach_events
			.iter()
			.find(|event| event.id == event_id)
			.map(|event| (event.start_idx, event.end_idx))
	}

	pub fn positions_wrt_planet_flyby(&self, 
		planet_id: i32
	) -> Option<Vec<DynamicTrajectorySegmentData>> {
		let central = match self.coord_system {
			simulation::CoordSystem::BodyCentric => 10,
			simulation::CoordSystem::BarocenterCentric => 0,
		};

		let planet_state_at = |et: f64| {
			if let Ok(state_vector) = utils::celestial_state_et(et, planet_id, central, "ECLIPJ2000") {
				state_vector
			} else {
				StateVector::default()
			}
		};

		let mut flyby_segments = Vec::new();
		for event in self.closest_approach_events.iter().filter(|event| event.body_id == planet_id) {
			let (start_idx, end_idx) = (event.start_idx, event.end_idx);
			let mut positions_wrt_planet = Vec::new();
			for i in start_idx..=end_idx {
				let state_at_epoch = self.states[i];
				let et = state_at_epoch.et;
				let planet_state = planet_state_at(et);
				let rel_state = [
					(state_at_epoch.state.position[0] - planet_state.position[0]) as f32,
					(state_at_epoch.state.position[1] - planet_state.position[1]) as f32,
					(state_at_epoch.state.position[2] - planet_state.position[2]) as f32,
				];
				positions_wrt_planet.push(rel_state);
			}

			if let (Ok(start_u32), Ok(end_u32)) = (start_idx.try_into(), end_idx.try_into()) {
				flyby_segments.push(DynamicTrajectorySegmentData {
					id: event.id,
					positions: positions_wrt_planet,
					segment: (start_u32, end_u32),
				});
			}
		}

		if flyby_segments.is_empty() {
			None
		} else {
			Some(flyby_segments)
		}
	}

}

pub struct ImpactorProbability {

}

impl ImpactorProbability {
	
	pub fn calculate_preliminary() {

	}
}
