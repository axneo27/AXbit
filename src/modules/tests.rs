#[cfg(test)]
pub static SPICE_TEST_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[cfg(test)]
mod sbdb_api_tests {
    use crate::modules::sbdb;

    #[test]
    fn parses_close_approach_response() {
        let data = serde_json::json!({
            "count": 1,
            "fields": ["des", "orbit_id", "jd", "cd", "dist", "dist_min", "dist_max", "v_rel", "v_inf", "t_sigma_f", "h", "fullname"],
            "data": [["99942", "220", "2462240.407", "2029-Apr-13 21:46", "0.000254", "0.000253", "0.000255", "7.4225", "5.84", "< 00:01", "19.09", "99942 Apophis"]]
        });

        let results = sbdb::parse_cad_response(&data, "Earth").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].designation, "99942");
        assert_eq!(results[0].sb_name, "99942 Apophis");
        assert_eq!(results[0].jpl_orbit_id, "220");
        assert_eq!(results[0].nominal_distance_au, 0.000254);
        assert_eq!(results[0].time_uncertainty.as_deref(), Some("< 00:01"));
    }

    #[test]
    fn shifts_fullname_when_all_bodies_are_requested() {
        let data = serde_json::json!({
            "count": 1,
            "fields": ["des", "orbit_id", "jd", "cd", "dist", "dist_min", "dist_max", "v_rel", "v_inf", "t_sigma_f", "body", "h", "fullname"],
            "data": [["99942", "220", "2462240.407", "2029-Apr-13 21:46", "0.000254", "0.000253", "0.000255", "7.4225", "5.84", "< 00:01", "Earth", "19.09", "99942 Apophis"]]
        });

        let results = sbdb::parse_cad_response(&data, "ALL").unwrap();
        assert_eq!(results[0].encounter_body, "Earth");
        assert_eq!(results[0].sb_name, "99942 Apophis");
    }

    #[test]
    fn accepts_empty_close_approach_response() {
        let data = serde_json::json!({"count": 0});
        assert!(sbdb::parse_cad_response(&data, "Earth").unwrap().is_empty());
    }
}

#[cfg(test)]
mod unit_conversion_tests {
    use crate::modules::{projection_3d::simulation::AU_KM, utils};

    #[test]
    fn converts_au_and_km() {
        assert_eq!(utils::au_to_km(1.0), AU_KM);
        assert_eq!(utils::km_to_au(AU_KM), 1.0);
    }

    #[test]
    fn au_km_conversion_round_trip() {
        let distance_au = 0.000254090910419299;
        let converted = utils::km_to_au(utils::au_to_km(distance_au));

        assert!((converted - distance_au).abs() < 1.0e-15);
    }
}

#[cfg(test)]
fn required_test_kernels_available() -> bool {
    let config_text = std::fs::read_to_string("kernels.toml")
        .expect("could not read kernels.toml");
    let config: crate::modules::kernel_config::KernelConfig =
        toml::from_str(&config_text).expect("could not parse kernels.toml");
    let kernels_path = std::path::Path::new("spice-tools/kernels");

    let mut missing = config
        .base_kernels
        .kernels
        .iter()
        .map(|kernel| &kernel.file)
        .filter(|file| !kernels_path.join(file).is_file())
        .cloned()
        .collect::<Vec<_>>();

    let inner = config
        .groups
        .get("inner_solar_system")
        .expect("kernels.toml has no inner_solar_system group");
    missing.extend(
        inner
            .kernels
            .iter()
            .map(|kernel| &kernel.file)
            .filter(|file| !kernels_path.join(file).is_file())
            .cloned(),
    );

    if missing.is_empty() {
        true
    } else {
        eprintln!(
            "skipping kernel-dependent test; missing: {}",
            missing.join(", ")
        );
        false
    }
}

#[cfg(test)]
mod spice_tests {
    use crate::modules::{projection_3d::simulation::Simulation, utils};
    use log::info;
    use crate::modules::{spice_bindings, spice_ker};
    use std::sync::MutexGuard;

    // Acquire the global SPICE lock and initialize kernels. The returned guard
    // keeps the lock held for the duration of the test, serializing SPICE calls.
    fn spice_setup() -> Option<MutexGuard<'static, ()>> {
        if !super::required_test_kernels_available() {
            return None;
        }
        let guard = super::SPICE_TEST_LOCK.lock().unwrap();
        spice_ker::initialize_test_kernels(std::path::Path::new("spice-tools/kernels"))
            .expect("could not initialize SPICE test kernels");
        Some(guard)
    }

    #[test]
    fn get_current_utc() {
        let Some(_g) = spice_setup() else { return };
        info!("Testing current_utc...");
        let res = utils::current_utc();
        assert!(res.is_some(), "current_utc returned None");
    }

    #[test]
    fn keplerian_elements() {
        let Some(_g) = spice_setup() else { return };
        let utc = utils::current_utc().expect("expected UTC");
        let et = utils::utc_to_et(&utc).expect("utc_to_et failed");
        
        let state = utils::celestial_state_utc(utc, 399, 0, Some("ECLIPJ2000".to_owned())).expect("celestial_state_utc failed");
        let gm = utils::mu(399).expect("mu failed");
        // pass epoch (ET converted from current UTC)
        let elements = utils::keplerian_elements(&state, et, gm)
            .expect("keplerian_elements failed");
        info!("Keplerian elements: {:?}", elements);
    }

    #[test]
    fn utc_to_et() {
        let Some(_g) = spice_setup() else { return };
        let et = utils::utc_to_et("2000-01-01 12:00:00").unwrap();
        info!("ET for 2000-01-01 12:00:00: {}", et);
    }

    #[test]
    fn spice_errors_are_returned_and_reset() {
        let Some(_g) = spice_setup() else { return };
        let error = utils::utc_to_et("not a UTC timestamp")
            .expect_err("invalid UTC should return a SPICE error");
        assert!(error.0.starts_with("SPICE("));

        let load_error = spice_bindings::spice_load_kernel(
            "spice-tools/kernels/does-not-exist.bsp",
        )
        .expect_err("missing kernel should return a SPICE error");
        assert!(load_error.starts_with("SPICE("));

        utils::utc_to_et("2000-01-01 12:00:00")
            .expect("SPICE should recover after the error is reset");
    }

    #[test]
    fn spice_query_errors_are_returned_and_reset() {
        let Some(_g) = spice_setup() else { return };
        let et = utils::utc_to_et("2000-01-01 12:00:00").unwrap();

        let state_error = utils::celestial_state_et(et, 123_456_789, 0, "ECLIPJ2000")
            .expect_err("an unknown target should return a SPICE error");
        assert!(state_error.0.starts_with("SPICE("));

        let mu_error = utils::mu(123_456_789)
            .expect_err("an unknown body's GM should return a SPICE error");
        assert!(mu_error.0.starts_with("SPICE("));

        utils::celestial_state_et(et, 399, 0, "ECLIPJ2000")
            .expect("SPICE should recover after query errors are reset");
    }

    #[test]
    fn ephemeris_body_centric() {
        let Some(_g) = spice_setup() else { return };
        let s = Simulation::new(crate::modules::projection_3d::simulation::CoordSystem::BodyCentric).expect("Could not initialize Simulation");
        assert!(!s.naif_celestial_objects.is_empty());
        info!("BodyCentric: {} celestial objects", s.naif_celestial_objects.len());

        for i in 1..=9 {
            let id = i * 100 + 99;
            if let Some(body) = s.naif_celestial_objects.get(&id) {
                info!("Body {} (ID {}): {} constituents", body.name, id, body.constituents_ids.len());
            }
        }
        if let Some(sun) = s.naif_celestial_objects.get(&10) {
            if let Some(params) = sun.physical_params {
                let radii = params.radii;
                info!("sun radii: {}, {}, {}", radii[0], radii[1], radii[2]);
            }
        }
    }

    #[test]
    fn ephemeris_barocenter_centric() {
        let Some(_g) = spice_setup() else { return };
        let s = Simulation::new(crate::modules::projection_3d::simulation::CoordSystem::BarocenterCentric).expect("Could not initialize Simulation");
        assert!(!s.naif_celestial_objects.is_empty());
        if let Some(ssb) = s.naif_celestial_objects.get(&0) {
            info!("SSB constituents: {}", ssb.constituents_ids.len());
        }
        utils::clear_spice_m();
    }

    #[test]
    #[ignore = "Run this manually. Not with other tests."]
    fn dump_loaded_body_ids_with_gm_to_file() {
        let Some(_g) = spice_setup() else { return };

        spice_ker::initialize_kernel_registry(
            std::path::Path::new("kernels.toml"),
            std::path::Path::new("spice-tools/kernels"),
        )
        .expect("failed to initialize kernel registry");

        let groups = spice_ker::get_available_groups().expect("failed to get available groups");
        for group_id in &groups {
            spice_ker::load_kernel_group(group_id)
                .unwrap_or_else(|error| panic!("failed to load group '{}': {}", group_id, error));
        }

        let mut ids: Vec<i32> = Vec::new();
        for group_id in &groups {
            let (_name, _time_bounds, group_ids) = spice_ker::get_group_info(group_id)
                .unwrap_or_else(|error| panic!("failed to get group info for '{}': {}", group_id, error));
            for id in group_ids {
                if utils::mu(id).is_ok() {
                    ids.push(id);
                }
            }
        }

        ids.sort_unstable();
        ids.dedup();

        let mut output = String::new();
        for id in &ids {
            output.push_str(&id.to_string());
            output.push('\n');
        }

        std::fs::write("gms.txt", output).expect("failed to write file1.txt");
        info!("Wrote {} ids with GM to gms.txt", ids.len());
    }

}

#[cfg(test)]
mod spice_ker_tests {
    use log::info;
    use crate::modules::{spice_bindings, spice_ker};
    use std::sync::MutexGuard;
    use crate::modules::spice_ker::{
        initialize_kernel_registry, load_kernel_group, unload_kernel_group, 
        get_available_groups, get_group_info, is_group_loaded, list_loaded_groups,
        get_current_time_bounds, expand_id_ranges, naif_id_is_planet,
        naif_id_is_satellite,
    };

    fn spice_ker_setup() -> Option<MutexGuard<'static, ()>> {
        if !super::required_test_kernels_available() {
            return None;
        }
        let guard = super::SPICE_TEST_LOCK.lock().unwrap();
        initialize_registry().expect("failed to initialize kernel registry");
        Some(guard)
    }

    fn initialize_registry() -> Result<(), String> {
        spice_bindings::spice_clear().map_err(|error| error.to_string())?;
        spice_ker::furnish_base_kernels(
            std::path::Path::new("kernels.toml"),
            std::path::Path::new("spice-tools/kernels"),
        )?;
        initialize_kernel_registry(
            std::path::Path::new("kernels.toml"),
            std::path::Path::new("spice-tools/kernels"),
        )
    }

    #[test]
    fn test_kernel_registry_initialization() {
        env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Info)
            .try_init()
            .ok();

        let Some(_guard) = spice_ker_setup() else { return };
        info!("Kernel registry initialized successfully");
    }

    #[test]
    fn test_available_groups_discovery() {
        let Some(_guard) = spice_ker_setup() else { return };
        
        let groups = get_available_groups().expect("Failed to get available groups");
        println!("Available groups: {:?}", groups);
        info!("Available groups: {:?}", groups);
        assert!(!groups.is_empty(), "Should have at least one available group");
    }

    #[test]
    fn test_group_info_retrieval() {
        let Some(_guard) = spice_ker_setup() else { return };
        
        let groups = get_available_groups().expect("Failed to get available groups");
        
        if let Some(group_id) = groups.first() {
            let (name, time_bounds, ids) = get_group_info(group_id)
                .expect("Failed to get group info");
            
            info!("Group: {}", name);
            info!("Time span: {} to {}", time_bounds.0, time_bounds.1);
            info!("IDs: {:?}", ids);
            
            assert!(!name.is_empty(), "Group name should not be empty");
            assert!(time_bounds.0.is_finite(), "Start time should be finite");
            assert!(time_bounds.1.is_finite(), "End time should be finite");
        }
    }

    #[test]
    fn test_load_unload_group() {
        let Some(_guard) = spice_ker_setup() else { return };
        
        let groups = get_available_groups().expect("Failed to get available groups");
        
        if let Some(group_id) = groups.first() {
            load_kernel_group(group_id).expect("Failed to load group");
            let is_loaded = is_group_loaded(group_id).expect("Failed to check if group is loaded");
            assert!(is_loaded, "Group should be loaded after load_kernel_group");
            
            let loaded = list_loaded_groups().expect("Failed to list loaded groups");
            assert!(loaded.contains(group_id), "Group should be in loaded groups list");
            
            unload_kernel_group(group_id).expect("Failed to unload group");
            let is_loaded = is_group_loaded(group_id).expect("Failed to check if group is loaded");
            assert!(!is_loaded, "Group should not be loaded after unload_kernel_group");
        }
    }

    #[test]
    fn test_time_bounds_aggregation() {
        let Some(_guard) = spice_ker_setup() else { return };
        
        let (start, end) = get_current_time_bounds().expect("Failed to get time bounds");
        assert_eq!(start, 0.0, "Time start should be 0 when no groups loaded");
        assert_eq!(end, 0.0, "Time end should be 0 when no groups loaded");
        
        let groups = get_available_groups().expect("Failed to get available groups");
        info!("Available groups: {:?}", groups);
        if groups.len() >= 2 {
            let group0_id = &groups[0];
            info!("Loading group 0: {}", group0_id);
            load_kernel_group(group0_id).expect("Failed to load group");
            let (start1, end1) = get_current_time_bounds().expect("Failed to get time bounds");
            let (_name0, (g0_start, g0_end), _ids0) = get_group_info(group0_id).expect("Failed to get group 0 info");
            info!("Group 0 ({}) bounds: {} to {}", group0_id, g0_start, g0_end);
            info!("Time bounds after loading group 0: {} to {}", start1, end1);
            assert!(start1.is_finite() && end1.is_finite(), "Time bounds should be finite");
            
            let group1_id = &groups[1];
            info!("Loading group 1: {}", group1_id);
            load_kernel_group(group1_id).expect("Failed to load group");
            let (start2, end2) = get_current_time_bounds().expect("Failed to get time bounds");
            let (_name1, (g1_start, g1_end), _ids1) = get_group_info(group1_id).expect("Failed to get group 1 info");
            info!("Group 1 ({}) bounds: {} to {}", group1_id, g1_start, g1_end);
            info!("Time bounds after loading group 1: {} to {}", start2, end2);
            assert!(start2.is_finite() && end2.is_finite(), "Time bounds should be finite");
            
            info!("Checking: start2 ({}) >= start1 ({})", start2, start1);
            assert!(start2 >= start1, "Combined start should be >= individual group start (intersection logic)");
        }
    }

    #[test]
    fn test_ids_expansion() {
        let ranges = vec![[1, 3], [5, 5], [10, 12]];
        let ids = expand_id_ranges(&ranges);
        
        info!("Expanded IDs: {:?}", ids);
        assert_eq!(ids, vec![1, 2, 3, 5, 10, 11, 12], "ID ranges should expand correctly");
    }

    #[test]
    fn test_provisional_satellite_ids_are_not_planets() {
        for id in [65199, 65299] {
            assert!(!naif_id_is_planet(id));
            assert!(naif_id_is_satellite(id));
        }
    }

    #[test]
    fn test_multiple_group_operations() {
        let Some(_guard) = spice_ker_setup() else { return };
        
        let groups = get_available_groups().expect("Failed to get available groups");
        
        if groups.len() >= 2 {
            let group_a = &groups[0];
            let group_b = &groups[1];
            
            load_kernel_group(group_a).expect("Failed to load group A");
            let loaded = list_loaded_groups().expect("Failed to list loaded groups");
            assert_eq!(loaded.len(), 1, "Should have 1 loaded group");
            
            load_kernel_group(group_b).expect("Failed to load group B");
            let loaded = list_loaded_groups().expect("Failed to list loaded groups");
            assert_eq!(loaded.len(), 2, "Should have 2 loaded groups");
            
            unload_kernel_group(group_a).expect("Failed to unload group A");
            let loaded = list_loaded_groups().expect("Failed to list loaded groups");
            assert_eq!(loaded.len(), 1, "Should have 1 loaded group after unloading group A");
            assert!(loaded.contains(group_b), "Group B should still be loaded");
            
            unload_kernel_group(group_b).expect("Failed to unload group B");
        }
    }
}
