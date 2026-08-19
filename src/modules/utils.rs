use std::{error::Error, ffi::{CStr, CString}, collections::HashMap, fmt, sync::{Mutex, LazyLock}, time::{SystemTime, UNIX_EPOCH}};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike};
use libc::c_char;
use log::{error, warn};
use crate::modules::{projection_3d::{simulation, state::{StateVector, Vec3d}}, spice_bindings};
use std::write;
use std::format;

#[derive(Debug, Clone, Copy)]
pub struct YMD {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl YMD {
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        Self { year, month, day }
    }

    pub fn date(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)
    }

    pub fn api_string(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct YMDHMS {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl YMDHMS {
    pub fn from_datetime(datetime: NaiveDateTime) -> Self {
        Self {
            year: datetime.year(),
            month: datetime.month(),
            day: datetime.day(),
            hour: datetime.hour(),
            minute: datetime.minute(),
            second: datetime.second(),
        }
    }

    pub fn utc_string(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DistanceUnit {
    AU,
    KM,
}

impl DistanceUnit {
    pub fn to_string(&self) -> &'static str {
        match self {
            DistanceUnit::AU => "au",
            DistanceUnit::KM => "km",
        }
    }
}

pub fn au_to_km(au: f64) -> f64 {
    au * simulation::AU_KM
}

pub fn km_to_au(km: f64) -> f64 {
    km / simulation::AU_KM
}

#[derive(Debug)]
pub struct SpiceError(pub String);

impl Error for SpiceError {}

impl fmt::Display for SpiceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SpiceError: {}", self.0)
    }
}

static IRREGULAR_BODIES: LazyLock<Mutex<HashMap<i32, [f64; 3]>>> = LazyLock::new(|| {
    let mut res = HashMap::new();
    res.insert(716, [36.0, 36.0, 36.0]);
    res.insert(717, [95.0, 70.0, 65.0]);
    res.insert(718, [25.0, 25.0, 25.0]);
    res.insert(719, [24.0, 24.0, 24.0]);
    res.insert(720, [10.0, 10.0, 10.0]);
    res.insert(721, [9.0, 9.0, 9.0]);
    res.insert(722, [11.0, 11.0, 11.0]);
    res.insert(723, [10.0, 10.0, 10.0]);
    res.insert(724, [10.0, 10.0, 10.0]);
    res.insert(725, [10.0, 10.0, 10.0]);
    res.insert(726, [5.0, 5.0, 5.0]);
    res.insert(727, [9.0, 9.0, 9.0]);

    res.insert(809, [31.0, 31.0, 31.0]);
    res.insert(810, [20.0, 20.0, 20.0]);
    res.insert(811, [22.0, 22.0, 22.0]);
    res.insert(812, [21.0, 21.0, 21.0]);
    res.insert(813, [30.0, 30.0, 30.0]);
    
    Mutex::new(res)
});

fn with_spice_lock<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    let _guard = spice_bindings::SPICE_LOCK.lock().expect("SPICE mutex poisoned");
    f()
}

fn status_to_result(status: i32, error: &[c_char; spice_bindings::SPICE_ERROR_SIZE]) -> Result<(), SpiceError> {
    if status == 0 {
        Ok(())
    } else {
        let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy().into_owned();
        Err(SpiceError(message))
    }
}

pub fn current_utc() -> Option<String> {
    let now = SystemTime::now();
    let duration = now
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime occurred before UNIX EPOCH");

    match DateTime::from_timestamp_millis(duration.as_millis() as i64) {
        Some(t) => Some(t.format("%Y-%m-%d %H:%M:%S%.3f").to_string()),
        None => None,
    }
}

pub fn keplerian_elements(state: &StateVector, et: f64, gm: f64) -> Result<[f64; 6], SpiceError> {
    let mut elements = [0.0; 6];
    let state_arr: [f64; 6] = (*state).into();
    let mut error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::state2keplerian(state_arr.as_ptr(), et, gm, elements.as_mut_ptr(), error.as_mut_ptr(), error.len())
    });
    status_to_result(inner_res, &error)?;
    Ok(elements)
}

pub fn utc_to_et(utc: &str) -> Result<f64, SpiceError> {
    let c_utc = CString::new(utc)
        .map_err(|_| SpiceError("UTC string contains an interior NUL byte".to_string()))?;
    let mut et = 0.0;
    let mut error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::utc2et(c_utc.as_ptr(), &mut et, error.as_mut_ptr(), error.len())
    });
    status_to_result(inner_res, &error)?;
    Ok(et)
}

pub fn et_to_utc(et: f64) -> Result<String, SpiceError> {
    let pictur = CString::new("YYYY-MM-DD HR:MN:SC.###").unwrap();
    let mut output = [0 as c_char; 50];
    let mut error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::timout(et, pictur.as_ptr(), output.len(), output.as_mut_ptr(), error.as_mut_ptr(), error.len())
    });
    status_to_result(inner_res, &error)?;
    Ok(unsafe { CStr::from_ptr(output.as_ptr()).to_string_lossy().into_owned() })
}

pub fn format_utc_display(utc: &str) -> &str {
    if utc.len() >= 19 { &utc[..19] } else { utc }
}

/// Converts a Julian Ephemeris Date to ET seconds past J2000.
pub fn jd_to_et(jd: f64) -> Result<f64, SpiceError> {
    let mut et = 0.0;
    let mut error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::jd_to_et(jd, &mut et, error.as_mut_ptr(), error.len())
    });
    status_to_result(inner_res, &error)?;
    Ok(et)
}

pub fn celestial_state_utc(
    time_utc: String, 
    target: i32, 
    observer: i32, 
    ref_frame: Option<String>) -> Result<StateVector, SpiceError> {
	let rf = ref_frame.unwrap_or_else(|| "ECLIPJ2000".to_string());
	let et = utc_to_et(&time_utc)?;
    celestial_state_et(et, target, observer, &rf)
}

pub fn celestial_state_et(
    time_et: f64, 
    target: i32, 
    observer: i32, 
    ref_frame: &str) -> Result<StateVector, SpiceError> {
    let c_ref_frame = CString::new(ref_frame)
        .map_err(|_| SpiceError("Reference frame contains an interior NUL byte".to_string()))?;
    let mut res = [0.0; 6];
    let mut error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::spkez(time_et, target, observer, c_ref_frame.as_ptr(), res.as_mut_ptr(), error.as_mut_ptr(), error.len())
    });
    status_to_result(inner_res, &error)?;
    Ok(StateVector {
        position: Vec3d::new(res[0], res[1], res[2]),
        velocity: Vec3d::new(res[3], res[4], res[5]),
    })
}

pub fn pos_rel_to_ssb_at_time(id: i32, et: f64, ref_frame: &str) -> Vec3d {
    celestial_state_et(et, id, 0, ref_frame).ok().map(|state| state.position).unwrap_or_else(|| Vec3d::new(0.0, 0.0, 0.0))
}

pub fn naif_obj_name(id: i32) -> Option<String> {
    let mut name_buffer = [0 as c_char; 100];
    let mut found = spice_bindings::SPICEFALSE;
    let mut error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::bodc2n(id, name_buffer.as_mut_ptr(), name_buffer.len(), &mut found, error.as_mut_ptr(), error.len())
    });
    if status_to_result(inner_res, &error).is_err() || found != spice_bindings::SPICETRUE {
        return None;
    }
    Some(unsafe { CStr::from_ptr(name_buffer.as_ptr()).to_string_lossy().into_owned() })
}

pub fn radii(id: i32) -> Result<[f64; 3], SpiceError> { // in km
    let mut radii = [0.0; 3];
    let mut spice_error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::bodvcd_radii(id, radii.as_mut_ptr(), spice_error.as_mut_ptr(), spice_error.len())
    });
    if let Err(spice_error) = status_to_result(inner_res, &spice_error) {
        match IRREGULAR_BODIES.lock().expect("Could not lock irregular bodies hashmap").get(&id) {
            Some(&r) => {
                return Ok(r);
            },
            None => {
                warn!("Don't have radii for id: {}", id);
            },
        }
        warn!("SPICE radii lookup failed: {} Falling back to default", spice_error);
        Ok([1.0, 1.0, 1.0])
    } else { Ok(radii) }
}

pub fn mu(id: i32) -> Result<f64, SpiceError> {
    let mut mu = 0.0;
    let mut error = [0; spice_bindings::SPICE_ERROR_SIZE];
    let inner_res = with_spice_lock(|| unsafe {
        spice_bindings::bodvcd_mu(id, &mut mu, error.as_mut_ptr(), error.len())
    });
    status_to_result(inner_res, &error)?;
    Ok(mu)
}

pub fn clear_spice_m() {
    if let Err(error) = spice_bindings::spice_clear() {
        error!("Could not clear SPICE state: {}", error);
    }
}
