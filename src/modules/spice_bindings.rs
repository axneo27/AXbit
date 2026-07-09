use libc::{c_char, c_double, c_int, size_t};

pub type SpiceInt = c_int;
pub type SpiceDouble = c_double;
pub type SpiceBoolean = c_int; // CSPICE maps SpiceBoolean to int
pub const SPICETRUE: SpiceBoolean = 1;
pub const SPICEFALSE: SpiceBoolean = 0;
pub const SPICE_ERROR_SIZE: usize = 2048;

use std::ffi::{CStr, CString};
use std::sync::{Mutex, LazyLock};

pub static SPICE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn status_to_result(status: c_int, error: &[c_char; SPICE_ERROR_SIZE]) -> Result<(), String> {
	if status == 0 {
		Ok(())
	} else {
		let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy().into_owned();
		Err(message)
	}
}

pub fn spice_load_kernel(kernel_path: &str) -> Result<(), String> {
	let c_path = CString::new(kernel_path)
		.map_err(|_| "Kernel path contains an interior NUL byte".to_string())?;

	let _guard = SPICE_LOCK.lock().expect("SPICE mutex poisoned");

	let mut error = [0; SPICE_ERROR_SIZE];

	let status = unsafe { 
		load_kernel(c_path.as_ptr(), error.as_mut_ptr(), error.len()) 
	};

	status_to_result(status, &error)
}

pub fn spice_unload_kernel(kernel_path: &str) -> Result<(), String> {
	let c_path = CString::new(kernel_path)
		.map_err(|_| "Kernel path contains an interior NUL byte".to_string())?;

	let _guard = SPICE_LOCK.lock().expect("SPICE mutex poisoned");

	let mut error = [0; SPICE_ERROR_SIZE];

	let status = unsafe { 
		unload_kernel(c_path.as_ptr(), error.as_mut_ptr(), error.len())
	};

	status_to_result(status, &error)
}

pub fn spice_clear() -> Result<(), String> {
	let _guard = SPICE_LOCK.lock().expect("SPICE mutex poisoned");

	let mut error = [0; SPICE_ERROR_SIZE];

	let status = unsafe { 
		clear_spice(error.as_mut_ptr(), error.len()) 
	};

	status_to_result(status, &error)
}

unsafe extern "C" {
	pub fn state2keplerian(state: *const c_double, et: c_double, gm: c_double, elements: *mut c_double, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn utc2et(utc: *const c_char, et: *mut c_double, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn jd_to_et(jd: c_double, et: *mut c_double, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn timout(et: c_double, pictur: *const c_char, lenout: size_t, output: *mut c_char, error: *mut c_char, error_size: size_t) -> c_int;

	pub fn spkez(et: c_double, target: c_int, observer: c_int, ref_frame: *const c_char, state: *mut c_double, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn bodc2n(obj_id: SpiceInt, obj_name: *mut c_char, obj_name_size: size_t, found: *mut SpiceBoolean, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn bodvcd_radii(id: SpiceInt, radii: *mut c_double, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn bodvcd_mu(id: SpiceInt, mu: *mut c_double, error: *mut c_char, error_size: size_t) -> c_int;

	pub fn load_kernel(kernel_path: *const c_char, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn unload_kernel(kernel_path: *const c_char, error: *mut c_char, error_size: size_t) -> c_int;
	pub fn clear_spice(error: *mut c_char, error_size: size_t) -> c_int;
}
