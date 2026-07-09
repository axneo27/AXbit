use std::env;
pub use cc;
pub use anyhow::*;
use fs_extra::{copy_items, dir::CopyOptions};

fn main() -> Result<()> {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS")?;
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH")?;

    // Check if we have spice-tools/cspice/lib directory
    let cspice_lib_path = std::path::Path::new("spice-tools/cspice/lib");
    
    if !cspice_lib_path.exists() {
        return Err(anyhow::anyhow!("cspice lib directory not found: {}. Download it from https://naif.jpl.nasa.gov/naif/toolkit_C.html", cspice_lib_path.display()));
    }

    println!("cargo:rerun-if-changed=spice-tools/src/spice_utils.h");
    println!("cargo:rerun-if-changed=spice-tools/src/spice_utils.c");
    println!("cargo:rerun-if-changed=src/modules/projection_3d/res");

    cc::Build::new()
        .file("spice-tools/src/spice_utils.c")
        .include("spice-tools/cspice/include")
        .compile("axbit_spice");

    match (target_os.as_str(), target_arch.as_str()) {
        ("macos", "aarch64") | ("macos", "x86_64") | 
        ("linux", "x86_64") | ("windows", "x86_64") => {
            // All supported platforms use the same linking. (Currently tested only on macOS aarch64)
            println!("cargo:rustc-link-search=native=spice-tools/cspice/lib");
            println!("cargo:rustc-link-lib=static=cspice");
            println!("cargo:rustc-link-lib=static=csupport");
        }
        _ => {
            panic!("Unsupported target OS or architecture: {} {}", target_os, target_arch);
        }
    }

    let out_dir = env::var("OUT_DIR")?;
    let mut copy_options = CopyOptions::new();
    copy_options.overwrite = true;
    let mut paths_to_copy = Vec::new();
    paths_to_copy.push("src/modules/projection_3d/res");
    copy_items(&paths_to_copy, out_dir, &copy_options)?;

    Ok(())
}
