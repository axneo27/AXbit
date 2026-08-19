use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use crate::modules::utils::YMD;

pub const SBDB_API_URL: &str = "https://ssd-api.jpl.nasa.gov/sbdb.api";
pub const SBDB_API_URL_CLOSE_APPROACH: &str = "https://ssd-api.jpl.nasa.gov/cad.api";

// Default CAD response indexes for the query used below. When body=ALL, the
// API inserts the encounter body after t_sigma_f and shifts later fields by 1.
const CAD_DES: usize = 0;
const CAD_ORBIT_ID: usize = 1;
const CAD_JD: usize = 2;
const CAD_CALENDAR_DATE: usize = 3;
const CAD_DISTANCE: usize = 4;
const CAD_DISTANCE_MIN: usize = 5;
const CAD_DISTANCE_MAX: usize = 6;
const CAD_RELATIVE_VELOCITY: usize = 7;
const CAD_TIME_UNCERTAINTY: usize = 9;
const CAD_BODY: usize = 10;
const CAD_FULLNAME: usize = 11;

pub fn init_db() -> Result<()> {
    let conn = open_db()?;

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS celestial_bodies (
            -- Core identifiers
            id INTEGER PRIMARY KEY,
            spk_id TEXT UNIQUE NOT NULL,
            des TEXT,
            fullname TEXT,
            kind TEXT,

            -- Classification
            neo BOOLEAN,
            pha BOOLEAN,
            orbit_class_code TEXT,
            orbit_class_name TEXT,
            source TEXT,
            soln_date TEXT,

            -- Orbital elements (used by simulation)
            epoch_jd REAL,              -- JD epoch of elements
            eccentricity REAL,          -- e
            semi_major_axis_au REAL,    -- a (AU)
            inclination_deg REAL,       -- i (degrees)
            long_asc_node_deg REAL,     -- om (degrees)
            arg_perihelion_deg REAL,    -- w (degrees)
            mean_anomaly_deg REAL,      -- ma (degrees)

            -- Physical parameters
            diameter_km REAL,           -- computed from diameter or H/albedo
            gm_km3_s2 REAL,             -- GM (if available)
            h_magnitude REAL,           -- absolute magnitude H

            -- Complete JSON backup (for any fields we missed)
            raw_json TEXT NOT NULL,

            -- Metadata
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_spk_id ON celestial_bodies(spk_id);
        CREATE INDEX IF NOT EXISTS idx_des ON celestial_bodies(des);
        CREATE INDEX IF NOT EXISTS idx_neo ON celestial_bodies(neo);
        CREATE INDEX IF NOT EXISTS idx_epoch ON celestial_bodies(epoch_jd);
        CREATE INDEX IF NOT EXISTS idx_semi_major_axis ON celestial_bodies(semi_major_axis_au);
        CREATE INDEX IF NOT EXISTS idx_eccentricity ON celestial_bodies(eccentricity);
        ",
    )
    .context("failed to create database schema")?;

    Ok(())
}

fn open_db() -> Result<Connection> {
    let path = crate::modules::app_paths::get().sbdb();

    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).context("failed to create data directory")?;
        }
    }

    let conn = Connection::open(path).context("failed to open SQLite database")?;

    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
    ",
    )
    .context("failed to set pragmas")?;

    Ok(conn)
}

/// id can be either the SPK ID or the designation (des)
pub fn fetch_sbdb_object(id: &str) -> Result<Value> {
    let url = SBDB_API_URL;
    let resp = reqwest::blocking::Client::new()
        .get(url)
        .query(&[
            ("sstr", id),
            ("orbit-defs", "true"),
            ("full-prec", "true"),
            ("cd-epoch", "true"),
            ("phys-par", "true"),
            ("cov", "src"),
        ])
        .send()
        .with_context(|| format!("request to {} failed", url))?
        .error_for_status()
        .with_context(|| "non-success status returned from SBDB")?
        .json::<Value>()
        .with_context(|| "failed to parse SBDB response as JSON")?;

    Ok(resp)
}

pub fn search_cad_objects(filters: &CloseApproachFilters) -> Result<Vec<CloseApproachData>> {
    let url = SBDB_API_URL_CLOSE_APPROACH;
    let client = reqwest::blocking::Client::new();
    let start_date = filters.start_date.api_string();
    let end_date = filters.end_date.api_string();
    let maximum_distance_au = filters.maximum_distance_au.to_string();

    let response = client.get(url)
        .query(&[
            ("body", filters.encounter_body.as_deref().unwrap_or("ALL")),
            ("date-min", start_date.as_str()),
            ("date-max", end_date.as_str()),
            ("dist-max", maximum_distance_au.as_str()),
            ("fullname", "true"),
            ("limit", "200"),
        ])
        .send()
        .with_context(|| format!("request to {} failed", url))?;
    let status = response.status();
    let data = response
        .json::<Value>()
        .with_context(|| "failed to parse SBDB CAD search response as JSON")?;

    let requested_body = filters.encounter_body.as_deref().unwrap_or("Earth");
    parse_cad_response(&data, requested_body).with_context(|| format!(
        "SBDB CAD search failed ({}): {}",
        status,
        data["message"].as_str().unwrap_or("unexpected response")
    ))
}

pub(crate) fn parse_cad_response(data: &Value, requested_body: &str) -> Result<Vec<CloseApproachData>> {
    if data["count"].as_str() == Some("0") || data["count"].as_i64() == Some(0) {
        return Ok(vec![]);
    }

    let fields = data["fields"].as_array().context("CAD response did not include fields")?;
    let includes_body = fields.get(CAD_BODY).and_then(Value::as_str) == Some("body");
    let fullname_index = CAD_FULLNAME + usize::from(includes_body);

    let expected_fields = [
        (CAD_DES, "des"),
        (CAD_ORBIT_ID, "orbit_id"),
        (CAD_JD, "jd"),
        (CAD_CALENDAR_DATE, "cd"),
        (CAD_DISTANCE, "dist"),
        (CAD_DISTANCE_MIN, "dist_min"),
        (CAD_DISTANCE_MAX, "dist_max"),
        (CAD_RELATIVE_VELOCITY, "v_rel"),
        (CAD_TIME_UNCERTAINTY, "t_sigma_f"),
        (fullname_index, "fullname"),
    ];

    for (index, name) in expected_fields {
        if fields.get(index).and_then(Value::as_str) != Some(name) {
            anyhow::bail!("unexpected CAD response layout at index {}: expected '{}'", index, name);
        }
    }

    if let Some(list) = data["data"].as_array() {
        return list.iter().map(|object| {
            let object = object.as_array().context("CAD record was not an array")?;

            let designation = object.get(CAD_DES).and_then(Value::as_str)
                .context("CAD record did not include a designation")?
                .to_string();
            let fullname = object.get(fullname_index).and_then(Value::as_str)
                .unwrap_or(&designation)
                .trim()
                .to_string();

            let encounter_body = if includes_body {
                object.get(CAD_BODY).and_then(Value::as_str).unwrap_or(requested_body)
            } else {
                requested_body
            }.to_string();

            let tca_jd = object.get(CAD_JD).and_then(Value::as_str)
                .and_then(|value| value.parse::<f64>().ok())
                .context("CAD record did not include a valid JD")?;
            let nominal_distance_au = object.get(CAD_DISTANCE).and_then(Value::as_str)
                .and_then(|value| value.parse::<f64>().ok())
                .context("CAD record did not include a valid distance")?;
            let relative_velocity_km_s = object.get(CAD_RELATIVE_VELOCITY).and_then(Value::as_str)
                .and_then(|value| value.parse::<f64>().ok())
                .context("CAD record did not include a valid relative velocity")?;

            Ok(CloseApproachData {
                designation,
                sb_name: fullname,
                encounter_body,
                jpl_orbit_id: object.get(CAD_ORBIT_ID).and_then(Value::as_str).unwrap_or("").to_string(),
                tca_jd,
                tca_calendar: object.get(CAD_CALENDAR_DATE).and_then(Value::as_str).unwrap_or("").to_string(),
                nominal_distance_au,
                minimum_3sigma_distance_au: object.get(CAD_DISTANCE_MIN).and_then(Value::as_str)
                    .and_then(|value| value.parse::<f64>().ok()),
                maximum_3sigma_distance_au: object.get(CAD_DISTANCE_MAX).and_then(Value::as_str)
                    .and_then(|value| value.parse::<f64>().ok()),
                relative_velocity_km_s,
                time_uncertainty: object.get(CAD_TIME_UNCERTAINTY).and_then(Value::as_str).map(str::to_string),
            })
        }).collect();
    }

    anyhow::bail!(data["message"].as_str().unwrap_or("unexpected response").to_string())
}

pub fn search_sbdb_objects(search: &str) -> Result<Vec<SbdbSearchResult>> {
    let search = search.trim();
    if search.is_empty() {
        return Ok(vec![]);
    }

    let url = SBDB_API_URL;
    let client = reqwest::blocking::Client::new();
    let request = |search: &str| -> Result<(reqwest::StatusCode, Value)> {
        let response = client
            .get(url)
            .query(&[("sstr", search), ("no-orbit", "true")])
            .send()
            .with_context(|| format!("request to {} failed", url))?;
        let status = response.status();
        let data = response
            .json::<Value>()
            .with_context(|| "failed to parse SBDB search response as JSON")?;
        Ok((status, data))
    };

    let (mut status, mut data) = request(search)?;
    if data["code"].as_str() == Some("200") && data["message"].as_str().is_some_and(|message| message.contains("not found")) {
        (status, data) = request(&format!("{}*", search))?;
    }

    if let Some(list) = data["list"].as_array() {
        return Ok(list.iter().filter_map(|object| {
            let designation = object["pdes"].as_str()?.to_string();
            let name = object["name"].as_str().unwrap_or(&designation).trim().to_string();

            Some(SbdbSearchResult {
                id: designation.clone(),
                designation,
                name,
                description: None,
            })
        }).collect());
    }

    if let Some(object) = data.get("object") {
        let designation = object["des"].as_str().unwrap_or(search).to_string();
        let spk_id = object["spkid"].as_str().unwrap_or(&designation).to_string();
        let name = object["fullname"].as_str().unwrap_or(&designation).trim().to_string();
        let description = object["orbit_class"]["name"].as_str().map(str::to_string);

        return Ok(vec![SbdbSearchResult {
            id: spk_id,
            designation,
            name,
            description,
        }]);
    }

    if data["code"].as_str() == Some("200") && data["message"].as_str().is_some_and(|message| message.contains("not found")) {
        return Ok(vec![]);
    }

    anyhow::bail!(
        "SBDB search failed ({}): {}",
        status,
        data["message"].as_str().unwrap_or("unexpected response")
    )
}

pub fn diameter_from_h_albedo_m(h: f64, albedo: f64) -> f64 {
    let d_km = 1329.6 / albedo.sqrt() * 10f64.powf(-0.2 * h);
    d_km * 1000.0
}

pub fn download_and_store_small_body(search: &str) -> Result<i32> {
    let obj = fetch_sbdb_object(search).context("failed to fetch from SBDB API")?;
    let id = obj["object"]["spkid"]
        .as_str()
        .context("SBDB response did not include an SPK ID")?
        .parse::<i32>()
        .context("SBDB SPK ID is not a supported integer")?;

    store_body_from_json(id, &obj)?;
    Ok(id)
}

pub fn store_body_from_json(id: i32, data: &Value) -> Result<()> {
    let conn = open_db()?;

    // Extract metadata from JSON structure
    let obj = &data["object"];
    let orbit = &data["orbit"];

    let spk_id = obj["spkid"]
        .as_str()
        .unwrap_or(&id.to_string())
        .to_string();

    let des = obj["des"].as_str().unwrap_or("").to_string();
    let fullname = obj["fullname"].as_str().unwrap_or("").to_string();
    let kind = obj["kind"].as_str().unwrap_or("").to_string();
    let neo = obj["neo"].as_bool().unwrap_or(false);
    let pha = obj["pha"].as_bool().unwrap_or(false);

    let orbit_class_code = obj["orbit_class"]["code"].as_str().unwrap_or("");
    let orbit_class_name = obj["orbit_class"]["name"].as_str().unwrap_or("");

    let source = orbit["source"].as_str().unwrap_or("JPL").to_string();
    let soln_date = orbit["soln_date"].as_str().unwrap_or("");

    // Extract orbital elements from elements array
    let empty_elements = vec![];
    let elements = orbit["elements"].as_array().unwrap_or(&empty_elements);

    let find_elem = |name: &str| {
        elements
            .iter()
            .find(|el| el.get("name").and_then(|v| v.as_str()) == Some(name))
    };

    let epoch_jd: Option<f64> = orbit["epoch"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok());

    let eccentricity: Option<f64> = find_elem("e")
        .and_then(|el| el.get("value"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    let semi_major_axis_au: Option<f64> = find_elem("a")
        .and_then(|el| el.get("value"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    let inclination_deg: Option<f64> = find_elem("i")
        .and_then(|el| el.get("value"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    let long_asc_node_deg: Option<f64> = find_elem("om")
        .and_then(|el| el.get("value"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    let arg_perihelion_deg: Option<f64> = find_elem("w")
        .and_then(|el| el.get("value"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    let mean_anomaly_deg: Option<f64> = find_elem("ma")
        .and_then(|el| el.get("value"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    // Extract physical parameters from phys_par array
    let mut diameter_km: Option<f64> = None;
    let mut gm_km3_s2: Option<f64> = None;
    let mut h_magnitude: Option<f64> = None;
    let mut albedo: Option<f64> = None;

    if let Some(arr) = obj.get("phys_par").and_then(|v| v.as_array()) {
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
                            diameter_km = Some(d_km / 2.0); // diameter -> radius
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
                            h_magnitude = Some(h);
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

    // If no diameter but have H magnitude, compute it
    if diameter_km.is_none() && h_magnitude.is_some() {
        let h = h_magnitude.unwrap();
        let pv = albedo.unwrap_or(0.15);
        let d_m = diameter_from_h_albedo_m(h, pv);
        diameter_km = Some(d_m / 2000.0); // meters -> km, diameter -> radius
    }

    // Store full JSON for completeness
    let raw_json =
        serde_json::to_string(data).context("failed to serialize JSON to string")?;

    conn.execute(
        "
        INSERT OR REPLACE INTO celestial_bodies
        (id, spk_id, des, fullname, kind, neo, pha, orbit_class_code, orbit_class_name,
         source, soln_date, epoch_jd, eccentricity, semi_major_axis_au, inclination_deg,
         long_asc_node_deg, arg_perihelion_deg, mean_anomaly_deg, diameter_km,
         gm_km3_s2, h_magnitude, raw_json, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21, ?22, CURRENT_TIMESTAMP)
        ",
        params![
            id,
            spk_id,
            des,
            fullname,
            kind,
            neo,
            pha,
            orbit_class_code,
            orbit_class_name,
            source,
            soln_date,
            epoch_jd,
            eccentricity,
            semi_major_axis_au,
            inclination_deg,
            long_asc_node_deg,
            arg_perihelion_deg,
            mean_anomaly_deg,
            diameter_km,
            gm_km3_s2,
            h_magnitude,
            raw_json
        ],
    )
    .context("failed to insert/update celestial body in database")?;

    Ok(())
}

pub fn get_celestial_data(id: i32) -> Result<Value> {
    let conn = open_db()?;

    let raw_json: String = conn
        .query_row(
            "SELECT raw_json FROM celestial_bodies WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .context("database query failed")?
        .ok_or_else(|| anyhow::anyhow!("no downloaded data for id {}", id))?;

    serde_json::from_str(&raw_json).context("failed to deserialize stored JSON")
}

pub fn get_celestial_data_by_spk(spk_id: &str) -> Result<Value> {
    let conn = open_db()?;

    let raw_json: String = conn
        .query_row(
            "SELECT raw_json FROM celestial_bodies WHERE spk_id = ?1",
            params![spk_id],
            |row| row.get(0),
        )
        .optional()
        .context("database query failed")?
        .ok_or_else(|| anyhow::anyhow!("no downloaded data for spk_id {}", spk_id))?;

    serde_json::from_str(&raw_json).context("failed to deserialize stored JSON")
}

pub fn downloaded_ids() -> Result<Vec<i32>> {
    let conn = open_db()?;

    let mut stmt = conn
        .prepare("SELECT id FROM celestial_bodies ORDER BY id")
        .context("failed to prepare SELECT statement")?;

    let ids = stmt
        .query_map([], |row| row.get(0))
        .context("query execution failed")?
        .collect::<std::result::Result<Vec<i32>, _>>()
        .context("failed to collect ID rows")?;

    Ok(ids)
}

pub fn list_downloaded_bodies() -> Result<Vec<DownloadedBodyInfo>> {
    let conn = open_db()?;

    let mut stmt = conn
        .prepare(
            "SELECT id, spk_id, des, fullname, neo, pha, source, orbit_class_name,
                epoch_jd, eccentricity, semi_major_axis_au, inclination_deg,
                diameter_km, gm_km3_s2, h_magnitude, created_at, updated_at
         FROM celestial_bodies ORDER BY id",
        )
        .context("failed to prepare SELECT statement")?;

    let bodies = stmt
        .query_map([], |row| {
            Ok(DownloadedBodyInfo {
                id: row.get(0)?,
                spk_id: row.get(1)?,
                des: row.get(2)?,
                fullname: row.get(3)?,
                neo: row.get(4)?,
                pha: row.get(5)?,
                source: row.get(6)?,
                orbit_class_name: row.get(7)?,
                epoch_jd: row.get(8)?,
                eccentricity: row.get(9)?,
                semi_major_axis_au: row.get(10)?,
                inclination_deg: row.get(11)?,
                diameter_km: row.get(12)?,
                gm_km3_s2: row.get(13)?,
                h_magnitude: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
            })
        })
        .context("query execution failed")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to collect body rows")?;

    Ok(bodies)
}

pub fn delete_downloaded_small_bodies(ids: &[i32]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }

    let conn = open_db()?;
    let mut removed = 0usize;

    for id in ids {
        let rows_deleted = conn
            .execute(
                "DELETE FROM celestial_bodies WHERE id = ?1",
                params![id],
            )
            .context("failed to delete body from database")?;

        if rows_deleted > 0 {
            removed += 1;
        }
    }

    Ok(removed)
}

pub fn is_neo(id: i32) -> Result<bool> {
    let conn = open_db()?;

    let neo: bool = conn
        .query_row(
            "SELECT neo FROM celestial_bodies WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .context("database query failed")?
        .unwrap_or(false);

    Ok(neo)
}

pub fn is_downloaded(id: i32) -> Result<bool> {
    let conn = open_db()?;

    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM celestial_bodies WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .context("database query failed")?;

    Ok(exists)
}

pub fn downloaded_count() -> Result<usize> {
    let conn = open_db()?;

    let count: usize = conn
        .query_row("SELECT COUNT(*) FROM celestial_bodies", [], |row| {
            row.get(0)
        })
        .context("database query failed")?;

    Ok(count)
}

pub fn clear_all() -> Result<()> {
    let conn = open_db()?;
    conn.execute("DELETE FROM celestial_bodies", [])
        .context("failed to clear all bodies from database")?;
    Ok(())
}

pub fn redownload_all_bodies() -> Result<usize> {
    let ids = downloaded_ids().context("failed to load current downloaded body ids")?;
    let mut refreshed = 0;

    for id in ids {
        let id_str = id.to_string();
        match fetch_sbdb_object(&id_str)
            .and_then(|obj| store_body_from_json(id, &obj))
        {
            Ok(()) => refreshed += 1,
            Err(e) => log::warn!("Failed to re-download body {}: {}", id, e),
        }
    }

    Ok(refreshed)
}

#[derive(Clone, Debug)]
pub struct DownloadedBodyInfo {
    pub id: i32,
    pub spk_id: String,
    pub des: String,
    pub fullname: String,
    pub neo: bool,
    pub pha: bool,
    pub source: String,
    pub orbit_class_name: String,

    // Orbital elements
    pub epoch_jd: Option<f64>,
    pub eccentricity: Option<f64>,
    pub semi_major_axis_au: Option<f64>,
    pub inclination_deg: Option<f64>,

    // Physical properties
    pub diameter_km: Option<f64>,
    pub gm_km3_s2: Option<f64>,
    pub h_magnitude: Option<f64>,

    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub struct SbdbSearchResult {
    pub id: String,
    pub designation: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CloseApproachData {
    pub designation: String,
    pub sb_name: String,
    pub encounter_body: String,
    pub jpl_orbit_id: String,
    pub tca_jd: f64,
    pub tca_calendar: String,
    pub nominal_distance_au: f64,
    pub minimum_3sigma_distance_au: Option<f64>,
    pub maximum_3sigma_distance_au: Option<f64>,
    pub relative_velocity_km_s: f64,
    /// e.g. could be "13:02" or "2_09:08" (2 days, 9 hours, 8 minutes)
    pub time_uncertainty: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CloseApproachFilters {
    /// either one or * or all
    pub encounter_body: Option<String>,
    pub start_date: YMD,
    pub end_date: YMD,
    pub maximum_distance_au: f64,
    pub downloaded_only: bool,
}

impl CloseApproachFilters {
    pub fn new() -> Self {
        Self {
            encounter_body: None,
            start_date: YMD::new(2026, 1, 1),
            end_date: YMD::new(2031, 1, 1),
            maximum_distance_au: 0.05,
            downloaded_only: false,
        }
    }
}

pub fn planet_name_to_cad_name(planet_name: &str) -> Option<&'static str> {
    match planet_name.to_lowercase().as_str() {
        "mercury" => Some("Merc"),
        "venus" => Some("Venus"),
        "earth" => Some("Earth"),
        "mars" => Some("Mars"),
        "jupiter" => Some("Juptr"),
        "saturn" => Some("Satrn"),
        "uranus" => Some("Urnus"),
        "neptune" => Some("Neptn"),
        "pluto" => Some("Pluto"),
        "moon" => Some("Moon"),
        _ => None,
    }
}
