use crate::modules::utils::{
    json_f64_field, json_string_field, optional_json_f64_field,
    optional_json_string_field, YMD,
};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashMap;

pub const SBDB_API_URL: &str = "https://ssd-api.jpl.nasa.gov/sbdb.api";
pub const SBDB_API_URL_CLOSE_APPROACH: &str = "https://ssd-api.jpl.nasa.gov/cad.api";

const SCHEMA_VERSION: i64 = 1;

pub fn init_db() -> Result<()> {
    let mut conn = open_db()?;
    migrate_db(&mut conn)
}

pub(crate) fn migrate_db(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .context("failed to read database schema version")?;

    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version > SCHEMA_VERSION {
        anyhow::bail!(
            "database schema version {} is newer than supported version {}",
            version,
            SCHEMA_VERSION
        );
    }

    let tx = conn
        .transaction()
        .context("failed to start database migration")?;

    tx.execute_batch(
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

            -- Complete JSON backup (for any missed fields)
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

        CREATE TABLE IF NOT EXISTS close_approaches (
            id INTEGER PRIMARY KEY,
            celestial_body_id INTEGER NOT NULL,
            encounter_body TEXT NOT NULL,
            jpl_orbit_id TEXT NOT NULL,
            tca_jd REAL NOT NULL,
            tca_calendar TEXT NOT NULL,
            nominal_distance_au REAL NOT NULL,
            minimum_3sigma_distance_au REAL,
            maximum_3sigma_distance_au REAL,
            relative_velocity_km_s REAL NOT NULL,
            time_uncertainty TEXT,
            FOREIGN KEY (celestial_body_id)
                REFERENCES celestial_bodies(id) ON DELETE CASCADE,
            UNIQUE (celestial_body_id, encounter_body, jpl_orbit_id, tca_jd)
        );

        CREATE INDEX IF NOT EXISTS idx_close_approaches_owner
            ON close_approaches(celestial_body_id);
        CREATE INDEX IF NOT EXISTS idx_close_approaches_tca
            ON close_approaches(tca_jd);
        ",
    )
    .context("failed to create database schema")?;

    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
        .context("failed to update database schema version")?;
    tx.commit().context("failed to commit database migration")?;

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
            ("ca-data", "true"),
            ("ca-time", "both"),
            ("ca-tunc", "fmt"),
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

    let response = client
        .get(url)
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

    let requested_body = filters.encounter_body.as_deref().unwrap_or("ALL");
    parse_cad_response(&data, Some(requested_body)).with_context(|| {
        format!(
            "SBDB CAD search failed ({}): {}",
            status,
            data["message"].as_str().unwrap_or("unexpected response")
        )
    })
}

/// Finds a column in the field list that describes JPL CAD data rows.
///
/// The API returns records as arrays and provides their layout separately in
/// `fields`, so indexes must be resolved from each response instead of assumed.
fn find_field(fields: &[Value], name: &str) -> Result<usize> {
    fields
        .iter()
        .position(|field| field.as_str() == Some(name))
        .with_context(|| format!("CAD response did not include field '{}'", name))
}

pub(crate) fn parse_cad_response(
    data: &Value,
    requested_body: Option<&str>,
) -> Result<Vec<CloseApproachData>> {
    if data["count"].as_str() == Some("0") || data["count"].as_i64() == Some(0) {
        return Ok(vec![]);
    }

    let fields = data["fields"]
        .as_array()
        .context("CAD response did not include fields")?;
    let des = find_field(fields, "des")?;
    let orbit_id = find_field(fields, "orbit_id")?;
    let jd = find_field(fields, "jd")?;
    let calendar_date = find_field(fields, "cd")?;
    let distance = find_field(fields, "dist")?;
    let distance_min = find_field(fields, "dist_min")?;
    let distance_max = find_field(fields, "dist_max")?;
    let relative_velocity = find_field(fields, "v_rel")?;
    let time_uncertainty = find_field(fields, "t_sigma_f")?;
    let body = fields
        .iter()
        .position(|field| field.as_str() == Some("body"));
    let fullname = fields
        .iter()
        .position(|field| field.as_str() == Some("fullname"));

    if let Some(list) = data["data"].as_array() {
        return list
            .iter()
            .map(|object| {
                let object = object.as_array().context("CAD record was not an array")?;

                let designation = object
                    .get(des)
                    .and_then(Value::as_str)
                    .context("CAD record did not include a designation")?
                    .to_string();
                let fullname = fullname
                    .and_then(|index| object.get(index))
                    .and_then(Value::as_str)
                    .unwrap_or(&designation)
                    .trim()
                    .to_string();

                let encounter_body = if let Some(body) = body {
                    object
                        .get(body)
                        .and_then(Value::as_str)
                        .unwrap_or(requested_body.unwrap_or(""))
                } else {
                    requested_body.unwrap_or("")
                }
                .to_string();

                let tca_jd = object
                    .get(jd)
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<f64>().ok())
                    .context("CAD record did not include a valid JD")?;
                let nominal_distance_au = object
                    .get(distance)
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<f64>().ok())
                    .context("CAD record did not include a valid distance")?;
                let relative_velocity_km_s = object
                    .get(relative_velocity)
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<f64>().ok())
                    .context("CAD record did not include a valid relative velocity")?;

                Ok(CloseApproachData {
                    designation: Some(designation),
                    sb_name: Some(fullname),
                    encounter_body,
                    jpl_orbit_id: object
                        .get(orbit_id)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    tca_jd,
                    tca_calendar: object
                        .get(calendar_date)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    nominal_distance_au,
                    minimum_3sigma_distance_au: object
                        .get(distance_min)
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<f64>().ok()),
                    maximum_3sigma_distance_au: object
                        .get(distance_max)
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<f64>().ok()),
                    relative_velocity_km_s,
                    time_uncertainty: object
                        .get(time_uncertainty)
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect();
    }

    anyhow::bail!(data["message"]
        .as_str()
        .unwrap_or("unexpected response")
        .to_string())
}

pub(crate) fn parse_sbdb_close_approaches(data: &Value) -> Result<Vec<CloseApproachData>> {
    let Some(list) = data.get("ca_data") else {
        return Ok(vec![]);
    };
    let list = list
        .as_array()
        .context("SBDB ca_data field was not an array")?;

    // Keep this Value-based: JPL encodes numbers as strings.
    // Typed deserialization would need custom converters while still leaving us
    // with a second dynamic parser.
    list.iter()
        .map(|object| {
            Ok(CloseApproachData {
                designation: None,
                sb_name: None,
                encounter_body: json_string_field(object, "body")?,
                jpl_orbit_id: json_string_field(object, "orbit_ref")?,
                tca_jd: json_f64_field(object, "jd")?,
                tca_calendar: json_string_field(object, "cd")?,
                nominal_distance_au: json_f64_field(object, "dist")?,
                minimum_3sigma_distance_au: optional_json_f64_field(object, "dist_min"),
                maximum_3sigma_distance_au: optional_json_f64_field(object, "dist_max"),
                relative_velocity_km_s: json_f64_field(object, "v_rel")?,
                time_uncertainty: optional_json_string_field(object, "sigma_tf"),
            })
        })
        .collect()
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
    if data["code"].as_str() == Some("200")
        && data["message"]
            .as_str()
            .is_some_and(|message| message.contains("not found"))
    {
        (status, data) = request(&format!("{}*", search))?;
    }

    if let Some(list) = data["list"].as_array() {
        return Ok(list
            .iter()
            .filter_map(|object| {
                let designation = object["pdes"].as_str()?.to_string();
                let name = object["name"]
                    .as_str()
                    .unwrap_or(&designation)
                    .trim()
                    .to_string();

                Some(SbdbSearchResult {
                    id: designation.clone(),
                    designation,
                    name,
                    description: None,
                })
            })
            .collect());
    }

    if let Some(object) = data.get("object") {
        let designation = object["des"].as_str().unwrap_or(search).to_string();
        let spk_id = object["spkid"].as_str().unwrap_or(&designation).to_string();
        let name = object["fullname"]
            .as_str()
            .unwrap_or(&designation)
            .trim()
            .to_string();
        let description = object["orbit_class"]["name"].as_str().map(str::to_string);

        return Ok(vec![SbdbSearchResult {
            id: spk_id,
            designation,
            name,
            description,
        }]);
    }

    if data["code"].as_str() == Some("200")
        && data["message"]
            .as_str()
            .is_some_and(|message| message.contains("not found"))
    {
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
    let mut conn = open_db()?;

    // Extract metadata from JSON structure
    let obj = &data["object"];
    let orbit = &data["orbit"];

    let spk_id = obj["spkid"].as_str().unwrap_or(&id.to_string()).to_string();

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

    let epoch_jd: Option<f64> = orbit["epoch"].as_str().and_then(|s| s.parse::<f64>().ok());

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

    // SBDB documents `diameter` as an effective body diameter in km. Keep that
    // API value in the database; consumers that need a radius convert it at
    // the boundary.
    let mut diameter_km: Option<f64> = None;
    let mut gm_km3_s2: Option<f64> = None;
    let mut h_magnitude: Option<f64> = None;
    let mut albedo: Option<f64> = None;

    if let Some(arr) = data.get("phys_par").and_then(|v| v.as_array()) {
        for entry in arr {
            let pname = entry.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let val_str = entry.get("value").and_then(|v| v.as_str());

            match pname {
                "diameter" => {
                    if let Some(vs) = val_str {
                        if let Ok(d_km) = vs.parse::<f64>() {
                            diameter_km = Some(d_km);
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
        diameter_km = Some(d_m / 1000.0); // meters -> km
    }

    let close_approaches = parse_sbdb_close_approaches(data)
        .context("failed to parse SBDB close approaches")?;
    // Store full JSON for completeness
    let raw_json = serde_json::to_string(data).context("failed to serialize JSON to string")?;

    let tx = conn
        .transaction()
        .context("failed to start celestial body update")?;

    tx.execute(
        "
        INSERT INTO celestial_bodies
        (id, spk_id, des, fullname, kind, neo, pha, orbit_class_code, orbit_class_name,
         source, soln_date, epoch_jd, eccentricity, semi_major_axis_au, inclination_deg,
         long_asc_node_deg, arg_perihelion_deg, mean_anomaly_deg, diameter_km,
         gm_km3_s2, h_magnitude, raw_json, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21, ?22, CURRENT_TIMESTAMP)
        ON CONFLICT(id) DO UPDATE SET
            spk_id = excluded.spk_id,
            des = excluded.des,
            fullname = excluded.fullname,
            kind = excluded.kind,
            neo = excluded.neo,
            pha = excluded.pha,
            orbit_class_code = excluded.orbit_class_code,
            orbit_class_name = excluded.orbit_class_name,
            source = excluded.source,
            soln_date = excluded.soln_date,
            epoch_jd = excluded.epoch_jd,
            eccentricity = excluded.eccentricity,
            semi_major_axis_au = excluded.semi_major_axis_au,
            inclination_deg = excluded.inclination_deg,
            long_asc_node_deg = excluded.long_asc_node_deg,
            arg_perihelion_deg = excluded.arg_perihelion_deg,
            mean_anomaly_deg = excluded.mean_anomaly_deg,
            diameter_km = excluded.diameter_km,
            gm_km3_s2 = excluded.gm_km3_s2,
            h_magnitude = excluded.h_magnitude,
            raw_json = excluded.raw_json,
            updated_at = CURRENT_TIMESTAMP
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

    tx.execute(
        "DELETE FROM close_approaches WHERE celestial_body_id = ?1",
        params![id],
    )
    .context("failed to clear old close approaches")?;

    {
        let mut close_approach_insert = tx
            .prepare(
                "INSERT INTO close_approaches
                 (celestial_body_id, encounter_body, jpl_orbit_id, tca_jd,
                  tca_calendar, nominal_distance_au, minimum_3sigma_distance_au,
                  maximum_3sigma_distance_au, relative_velocity_km_s, time_uncertainty)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .context("failed to prepare close approach insert")?;

        for approach in close_approaches {
            close_approach_insert.execute(params![
                id,
                approach.encounter_body,
                approach.jpl_orbit_id,
                approach.tca_jd,
                approach.tca_calendar,
                approach.nominal_distance_au,
                approach.minimum_3sigma_distance_au,
                approach.maximum_3sigma_distance_au,
                approach.relative_velocity_km_s,
                approach.time_uncertainty,
            ])
            .context("failed to insert close approach")?;
        }
    }

    tx.commit()
        .context("failed to commit celestial body update")?;

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

    let mut downloaded_ids_query = conn
        .prepare("SELECT id FROM celestial_bodies ORDER BY id")
        .context("failed to prepare SELECT statement")?;

    let ids = downloaded_ids_query
        .query_map([], |row| row.get(0))
        .context("query execution failed")?
        .collect::<std::result::Result<Vec<i32>, _>>()
        .context("failed to collect ID rows")?;

    Ok(ids)
}

pub fn list_downloaded_bodies() -> Result<Vec<DownloadedBodyInfo>> {
    let conn = open_db()?;

    let mut downloaded_bodies_query = conn
        .prepare(
            "SELECT id, spk_id, des, fullname, neo, pha, source, orbit_class_name,
                epoch_jd, eccentricity, semi_major_axis_au, inclination_deg,
                diameter_km, gm_km3_s2, h_magnitude, created_at, updated_at
         FROM celestial_bodies ORDER BY id",
        )
        .context("failed to prepare SELECT statement")?;

    let mut bodies = downloaded_bodies_query
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
                ca_data: vec![],
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
            })
        })
        .context("query execution failed")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to collect body rows")?;

    let body_indexes = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| (body.id, index))
        .collect::<HashMap<_, _>>();

    let mut close_approaches_query = conn
        .prepare(
            "SELECT celestial_body_id, encounter_body, jpl_orbit_id, tca_jd,
                    tca_calendar, nominal_distance_au, minimum_3sigma_distance_au,
                    maximum_3sigma_distance_au, relative_velocity_km_s, time_uncertainty
             FROM close_approaches
             ORDER BY celestial_body_id, tca_jd",
        )
        .context("failed to prepare close approach SELECT statement")?;

    let approaches = close_approaches_query
        .query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                CloseApproachData {
                    designation: None,
                    sb_name: None,
                    encounter_body: row.get(1)?,
                    jpl_orbit_id: row.get(2)?,
                    tca_jd: row.get(3)?,
                    tca_calendar: row.get(4)?,
                    nominal_distance_au: row.get(5)?,
                    minimum_3sigma_distance_au: row.get(6)?,
                    maximum_3sigma_distance_au: row.get(7)?,
                    relative_velocity_km_s: row.get(8)?,
                    time_uncertainty: row.get(9)?,
                },
            ))
        })
        .context("close approach query failed")?;

    for approach in approaches {
        let (body_id, approach) = approach.context("failed to read close approach row")?;
        if let Some(&index) = body_indexes.get(&body_id) {
            bodies[index].ca_data.push(approach);
        }
    }

    Ok(bodies)
}

pub fn list_downloaded_close_approaches(filters: &CloseApproachFilters) -> Result<Vec<CloseApproachData>> {
    let conn: Connection = open_db()?;
    let use_encounter_body = filters.encounter_body.is_some(); 
    
    let mut close_approaches_query_string = 
        "   SELECT close_approaches.celestial_body_id, close_approaches.encounter_body, close_approaches.jpl_orbit_id,
            close_approaches.tca_jd, close_approaches.tca_calendar, close_approaches.nominal_distance_au, 
            close_approaches.minimum_3sigma_distance_au, close_approaches.maximum_3sigma_distance_au, 
            close_approaches.relative_velocity_km_s, close_approaches.time_uncertainty, 
            celestial_bodies.fullname, celestial_bodies.des 

            FROM close_approaches

            JOIN celestial_bodies ON close_approaches.celestial_body_id = celestial_bodies.id "
        .to_string();

    if use_encounter_body {
        close_approaches_query_string.push_str(" WHERE encounter_body = ?1 AND maximum_3sigma_distance_au <= ?2 ");
    } else {
        close_approaches_query_string.push_str(" WHERE maximum_3sigma_distance_au <= ?1 ");
    }
    close_approaches_query_string.push_str(" ORDER BY celestial_body_id, tca_jd");

    let mut close_approaches_query = conn
        .prepare(
            &close_approaches_query_string,
        )
        .context("failed to prepare close approach SELECT statement")?;

    let query_params = if use_encounter_body { 
        rusqlite::params![filters.encounter_body.clone().unwrap(), filters.maximum_distance_au]
        // by logic this should be safe to unwrap since we checked is_some() above
    } else {
        rusqlite::params![filters.maximum_distance_au]
    };
    
    let approaches = close_approaches_query
        .query_map(query_params, |row| {
            Ok((
                row.get::<_, i32>(0)?,
                CloseApproachData {
                    designation: row.get(11)?, // celestial_bodies.des with index 11
                    sb_name: row.get(10)?, // celestial_bodies.fullname with index 10 
                    encounter_body: row.get(1)?,
                    jpl_orbit_id: row.get(2)?,
                    tca_jd: row.get(3)?,
                    tca_calendar: row.get(4)?,
                    nominal_distance_au: row.get(5)?,
                    minimum_3sigma_distance_au: row.get(6)?,
                    maximum_3sigma_distance_au: row.get(7)?,
                    relative_velocity_km_s: row.get(8)?,
                    time_uncertainty: row.get(9)?,
                },
            ))
        })
        .context("close approach query failed")?;

    let mut filtered_approaches = Vec::new();
    for approach in approaches {
        let (_, approach) = approach.context("failed to read close approach row")?;
        // do not check if downloaded because we are already querying the downloaded close approaches table
        filtered_approaches.push(approach);
    }
    
    Ok(filtered_approaches)

}

pub fn delete_downloaded_small_bodies(ids: &[i32]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }

    let conn = open_db()?;
    let mut removed = 0usize;

    for id in ids {
        let rows_deleted = conn
            .execute("DELETE FROM celestial_bodies WHERE id = ?1", params![id])
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
        match fetch_sbdb_object(&id_str).and_then(|obj| store_body_from_json(id, &obj)) {
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

    pub ca_data: Vec<CloseApproachData>,

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

/// A singular close approach. The API's ca_data field is an array of these.
#[derive(Clone, Debug)]
pub struct CloseApproachData {
    /// Relevant when searching sbdb api, but not used inside the local DB object
    /// because we can find those by foreign ID
    pub designation: Option<String>,
    pub sb_name: Option<String>,

    pub encounter_body: String,
    pub jpl_orbit_id: String,
    pub tca_jd: f64,
    pub tca_calendar: String, // cd field in api
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
