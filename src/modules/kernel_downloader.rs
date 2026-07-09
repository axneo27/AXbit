use reqwest::{blocking::Client, header};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

use crate::modules::kernel_config::KernelConfig;

pub const NAIF_GENERIC_KERNELS_URL: &str =
    "https://naif.jpl.nasa.gov/pub/naif/generic_kernels/";
pub const KERNELS_CONFIG_FILE: &str = "kernels.toml";
pub const KERNELS_DIR: &str = "spice-tools/kernels";
/// Unused but kept for reference. 
/// We automatically load base_kernels before anything else, and inner_solar_system is the only other required group.
pub const REQUIRED_KERNEL_GROUPS: &[&str] = &["base_kernels", "inner_solar_system"];

pub fn kernels_dir() -> PathBuf {
    PathBuf::from(KERNELS_DIR)
}

pub fn kernels_config() -> PathBuf {
    PathBuf::from(KERNELS_CONFIG_FILE)
}

const USER_AGENT: &str = concat!("AXbit/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub struct KernelDownload {
    /// e.g. "lks/naif0012.tls"
    pub relative_path: PathBuf,
    /// e.g. "spice-tools/kernels/lks/naif0012.tls"
    pub destination: PathBuf,
    /// e.g. "https://naif.jpl.nasa.gov/pub/naif/generic_kernels/lks/naif0012.tls"
    pub url: reqwest::Url,
}

#[derive(Debug, Clone)]
pub struct KernelDownloadResume {
    relative_path: PathBuf,
    /// e.g. "spice-tools/kernels/lks/naif0012.tls.part"
    part_destination: PathBuf,
    /// e.g. "spice-tools/kernels/lks/naif0012.tls"
    final_destination: PathBuf,
    url: reqwest::Url,
    downloaded_bytes: u64,
}

impl KernelDownloadResume {
    /// Returns resume data only when `<destination>.part` is a non-empty file.
    pub fn from_download(download: &KernelDownload) -> Result<Option<Self>, KernelError> {
        let part_destination = PathBuf::from(format!("{}.part", download.destination.display()));
        let metadata = match fs::metadata(&part_destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(KernelError::FileSystem {
                    operation: FileOperation::Read,
                    path: part_destination,
                    source,
                });
            }
        };
        if !metadata.is_file() || metadata.len() == 0 {
            return Ok(None);
        }

        Ok(Some(Self {
            relative_path: download.relative_path.clone(),
            part_destination,
            final_destination: download.destination.clone(),
            url: download.url.clone(),
            downloaded_bytes: metadata.len(),
        }))
    }

    pub fn downloaded_bytes(&self) -> u64 {
        self.downloaded_bytes
    }
}

#[derive(Debug, Clone)]
pub struct KernelDownloadGroup {
    pub id: String,
    pub name: String,
    pub kernels: Vec<KernelDownload>,
    pub required: bool,
}

#[derive(Debug, Clone)]
pub struct KernelHeader {
    pub content_length: Option<u64>,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub accept_ranges: Option<String>,
}

#[derive(Debug, Clone)]
pub struct KernelFileStatus {
    pub relative_path: PathBuf,
    pub required: bool,
    pub is_available: bool,
    pub is_empty: bool,
}

#[derive(Debug, Clone)]
pub struct KernelDirectoryReport {
    pub files: Vec<KernelFileStatus>,
    pub incomplete_files: Vec<PathBuf>,
}

impl KernelDirectoryReport {
    pub fn is_ready(&self) -> bool {
        self.files.iter().all(|file| !file.required || file.is_available)
    }

    pub fn missing_required_files(&self) -> Vec<&KernelFileStatus> {
        self.files.iter()
            .filter(|file| file.required && !file.is_available)
            .collect()
    }
}

#[derive(Error, Debug, PartialEq, Eq, Clone, Copy)]
pub enum FileOperation {
    Create,
    Write,
    Read,
    Delete,
    Rename,
}

impl fmt::Display for FileOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileOperation::Create => write!(f, "create"),
            FileOperation::Write => write!(f, "write"),
            FileOperation::Read => write!(f, "read"),
            FileOperation::Delete => write!(f, "delete"),
            FileOperation::Rename => write!(f, "rename"),
        }
    }
}

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("failed to parse {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid kernel URL {url}: {source}")]
    InvalidUrl {
        url: String,
        source: url::ParseError,
    },
    #[error("failed to {operation} {path}: {source}")]
    FileSystem {
        operation: FileOperation,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("request failed for {url}: {source}")]
    Request {
        url: String,
        source: reqwest::Error,
    },
    #[error("downloaded {downloaded} bytes for {path}, expected {expected}")]
    SizeMismatch {
        path: PathBuf,
        downloaded: u64,
        expected: u64,
    },
    #[error("invalid Content-Range from {url}: expected byte {expected_start}, got {value}")]
    InvalidContentRange {
        url: String,
        expected_start: u64,
        value: String,
    },
    #[error("unexpected HTTP status {status} while resuming {url}")]
    UnexpectedResumeStatus {
        url: String,
        status: reqwest::StatusCode,
    },
}

pub fn create_client() -> Result<Client, KernelError> {
    Client::builder()
        .no_proxy()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|source| KernelError::Request {
            url: NAIF_GENERIC_KERNELS_URL.to_string(),
            source,
        })
}

pub fn read_kernel_groups(config_path: &Path, kernels_path: &Path) -> Result<Vec<KernelDownloadGroup>, KernelError> {
    let config = read_config(config_path)?;
    let base_url = reqwest::Url::parse(NAIF_GENERIC_KERNELS_URL)
        .map_err(|source| KernelError::InvalidUrl {
            url: NAIF_GENERIC_KERNELS_URL.to_string(),
            source,
        })?;
    let mut result = Vec::new();

    result.push(KernelDownloadGroup {
        id: "base_kernels".to_string(),
        name: "Base Kernels".to_string(),
        kernels: build_downloads(
            &config.base_kernels.kernels.into_iter().map(|kernel| kernel.file).collect::<Vec<String>>(),
            kernels_path,
            &base_url,
        )?,
        required: true,
    });

    for (id, group) in config.groups {
        let required = id == "inner_solar_system";
        let kernels = build_downloads(
            &group.kernels.into_iter().map(|kernel| kernel.file).collect::<Vec<String>>(),
            kernels_path,
            &base_url,
        )?;

        result.push(KernelDownloadGroup {
            id,
            name: group.name,
            kernels,
            required,
        });
    }

    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}

fn build_downloads(
    files: &[String],
    kernels_path: &Path,
    base_url: &reqwest::Url,
) -> Result<Vec<KernelDownload>, KernelError> {
    let mut kernels = Vec::new();
    for file in files {
        let relative_path = PathBuf::from(&file);
        let url = base_url.join(&file).map_err(|source| KernelError::InvalidUrl {
            url: file.to_string(),
            source,
        })?;
        kernels.push(KernelDownload {
            destination: kernels_path.join(&relative_path),
            relative_path,
            url,
        });
    }
    Ok(kernels)
}

/// Blocking. Call from a worker thread when used by the UI.
pub fn get_kernel_header(client: &Client, kernel: &KernelDownload) -> Result<KernelHeader, KernelError> {
    let response = client
        .head(kernel.url.clone())
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|source| KernelError::Request { url: kernel.url.to_string(), source })?
        .error_for_status()
        .map_err(|source| KernelError::Request { url: kernel.url.to_string(), source })?;

    let headers = response.headers();

    Ok(KernelHeader {
        content_length: headers
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok()),
        last_modified: header_value(headers, header::LAST_MODIFIED),
        etag: header_value(headers, header::ETAG),
        accept_ranges: header_value(headers, header::ACCEPT_RANGES),
    })
}

/// Blocking. Downloads only the kernel passed by the caller.
pub fn download_kernel<F: FnMut(u64, Option<u64>)>(client: &Client, kernel: &KernelDownload, mut on_progress: F) -> Result<u64, KernelError> {

    if let Some(parent) = kernel.destination.parent() {
        fs::create_dir_all(parent).map_err(|source| KernelError::FileSystem {
            operation: FileOperation::Create,
            path: parent.to_path_buf(),
            source,
        })?;
        // e.g. if we do not have spice-tools/kernels/lsk/naif0012.tls, 
        // we need to create spice-tools/kernels/lsk first
    }

    let final_path = kernel.destination.clone();
    let part_path = PathBuf::from(format!("{}.part", final_path.display()));

    let mut response = client
        .get(kernel.url.clone())
        .send()
        .map_err(|source| KernelError::Request { url: kernel.url.to_string(), source })?
        .error_for_status()
        .map_err(|source| KernelError::Request { url: kernel.url.to_string(), source })?;

    let expected_size = response.content_length();

    let mut output = File::create(&part_path).map_err(|source| KernelError::FileSystem {
        operation: FileOperation::Create,
        path: part_path.clone(),
        source,
    })?;
    
    let mut buffer = [0; 8192];
    let mut downloaded: u64 = 0;

    loop {
        let bytes_read = response.read(&mut buffer)
            .map_err(|source| KernelError::FileSystem {
                operation: FileOperation::Read,
                path: kernel.relative_path.clone(),
                source,
            })?;
        if bytes_read == 0 {
            break;
        }
        output.write_all(&buffer[..bytes_read]).map_err(|source| KernelError::FileSystem {
            operation: FileOperation::Write,
            path: part_path.clone(),
            source,
        })?;
        
        downloaded += bytes_read as u64;
        on_progress(downloaded, expected_size);
    }

    if let Some(expected_size) = expected_size {
        if downloaded != expected_size {
            return Err(KernelError::SizeMismatch {
                path: kernel.relative_path.clone(),
                downloaded,
                expected: expected_size,
            });
        }
    }

    // This cannot happen right?

    // if final_path.exists() {
    //     fs::remove_file(&final_path).map_err(|source| KernelError::FileSystem {
    //         operation: FileOperation::Delete,
    //         path: final_path.clone(),
    //         source,
    //     })?;
    // }

    drop(output);
    fs::rename(&part_path, &final_path).map_err(|source| KernelError::FileSystem {
        operation: FileOperation::Rename,
        path: final_path,
        source,
    })?;
    Ok(downloaded)
}

struct PreparedResumeTransfer {
    response: reqwest::blocking::Response,
    output: File,
    downloaded: u64,
    expected_size: Option<u64>,
}

fn prepare_resume_transfer(
    client: &Client,
    resume: &KernelDownloadResume,
) -> Result<PreparedResumeTransfer, KernelError> {
    let range_header_value = format!("bytes={}-", resume.downloaded_bytes);
    let response = client
        .get(resume.url.clone())
        .header(header::RANGE, range_header_value)
        .send()
        .map_err(|source| KernelError::Request {
            url: resume.url.to_string(),
            source,
        })?
        .error_for_status()
        .map_err(|source| KernelError::Request {
            url: resume.url.to_string(),
            source,
        })?;

    let response_length = response.content_length();
    match response.status() {
        reqwest::StatusCode::PARTIAL_CONTENT => {
            let content_range = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let (range_start, range_total) = parse_content_range(content_range).ok_or_else(|| {
                KernelError::InvalidContentRange {
                    url: resume.url.to_string(),
                    expected_start: resume.downloaded_bytes,
                    value: content_range.to_string(),
                }
            })?;
            if range_start != resume.downloaded_bytes {
                return Err(KernelError::InvalidContentRange {
                    url: resume.url.to_string(),
                    expected_start: resume.downloaded_bytes,
                    value: content_range.to_string(),
                });
            }

            let output = OpenOptions::new()
                .append(true)
                .open(&resume.part_destination)
                .map_err(|source| KernelError::FileSystem {
                    operation: FileOperation::Write,
                    path: resume.part_destination.clone(),
                    source,
                })?;
            let expected_size = range_total
                .or_else(|| response_length.map(|length| length + resume.downloaded_bytes));

            Ok(PreparedResumeTransfer {
                response,
                output,
                downloaded: resume.downloaded_bytes,
                expected_size,
            })
        }
        reqwest::StatusCode::OK => {
            // Range is unsupported. Reuse the full response and restart the part file.
            let output = File::create(&resume.part_destination).map_err(|source| {
                KernelError::FileSystem {
                    operation: FileOperation::Create,
                    path: resume.part_destination.clone(),
                    source,
                }
            })?;

            Ok(PreparedResumeTransfer {
                response,
                output,
                downloaded: 0,
                expected_size: response_length,
            })
        }
        status => Err(KernelError::UnexpectedResumeStatus {
            url: resume.url.to_string(),
            status,
        }),
    }
}

pub fn resume_download_kernel<F: FnMut(u64, Option<u64>)>(
    client: &Client,
    resume: &KernelDownloadResume,
    mut on_progress: F,
) -> Result<u64, KernelError> {
    let PreparedResumeTransfer {
        mut response,
        mut output,
        mut downloaded,
        expected_size,
    } = prepare_resume_transfer(client, resume)?;

    on_progress(downloaded, expected_size);

    let mut buffer = [0; 8192];
    loop {
        let bytes_read = response
            .read(&mut buffer)
            .map_err(|source| KernelError::FileSystem {
                operation: FileOperation::Read,
                path: resume.relative_path.clone(),
                source,
            })?;
        if bytes_read == 0 {
            break;
        }

        output.write_all(&buffer[..bytes_read]).map_err(|source| {
            KernelError::FileSystem {
                operation: FileOperation::Write,
                path: resume.part_destination.clone(),
                source,
            }
        })?;
        downloaded += bytes_read as u64;
        on_progress(downloaded, expected_size);
    }

    if let Some(expected_size) = expected_size {
        if downloaded != expected_size {
            return Err(KernelError::SizeMismatch {
                path: resume.relative_path.clone(),
                downloaded,
                expected: expected_size,
            });
        }
    }

    drop(output);
    fs::rename(&resume.part_destination, &resume.final_destination).map_err(|source| {
        KernelError::FileSystem {
            operation: FileOperation::Rename,
            path: resume.final_destination.clone(),
            source,
        }
    })?;
    Ok(downloaded)
}


pub fn verify_kernel_dir_integrity(config_path: &Path, kernels_path: &Path) -> Result<KernelDirectoryReport, KernelError> {
    let config = read_config(config_path)?;
    let mut files = Vec::new();

    for kernel in &config.base_kernels.kernels {
        files.push(file_status(&kernel.file, true, kernels_path)?);
    }

    for (id, group) in &config.groups {
        let required = id == "inner_solar_system";
        for kernel in &group.kernels {
            files.push(file_status(&kernel.file, required, kernels_path)?);
        }
    }

    let mut incomplete_files = Vec::new();
    find_incomplete_files(kernels_path, &mut incomplete_files)?;
    incomplete_files.sort();
    Ok(KernelDirectoryReport { files, incomplete_files })
}

fn read_config(config_path: &Path) -> Result<KernelConfig, KernelError> {
    let config_str = fs::read_to_string(config_path).map_err(|source| KernelError::FileSystem {
        operation: FileOperation::Read,
        path: config_path.to_path_buf(),
        source,
    })?;
    toml::from_str(&config_str).map_err(|source| KernelError::ConfigParse {
        path: config_path.to_path_buf(),
        source,
    })
}

fn file_status(file: &str, required: bool, kernels_path: &Path) -> Result<KernelFileStatus, KernelError> {
    let relative_path = PathBuf::from(file);
    let path = kernels_path.join(&relative_path);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(KernelError::FileSystem {
            operation: FileOperation::Read,
            path,
            source,
        }),
    };
    let is_empty = metadata.as_ref().is_some_and(|metadata| metadata.len() == 0);
    let is_available = metadata.is_some_and(|metadata| metadata.is_file() && metadata.len() > 0);
    Ok(KernelFileStatus { relative_path, required, is_available, is_empty })
}

fn find_incomplete_files(path: &Path, result: &mut Vec<PathBuf>) -> Result<(), KernelError> {
    if !path.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(path).map_err(|source| KernelError::FileSystem {
        operation: FileOperation::Read,
        path: path.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| KernelError::FileSystem {
            operation: FileOperation::Read,
            path: path.to_path_buf(),
            source,
        })?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            find_incomplete_files(&entry_path, result)?;
        } else if entry_path.extension().is_some_and(|extension| extension == "part") {
            result.push(entry_path);
        }
    }
    Ok(())
}

pub fn remove_incomplete_kernel_files(incomplete_file_dirs: &[PathBuf]) -> Result<(), KernelError> {
    for file_path in incomplete_file_dirs {
        if file_path.exists() {
            fs::remove_file(file_path).map_err(|source| KernelError::FileSystem {
                operation: FileOperation::Delete,
                path: file_path.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

fn header_value(headers: &header::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_owned)
}

fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, _end) = range.split_once('-')?;

    let start = start.parse::<u64>().ok()?;
    let total = if total == "*" {
        None
    } else {
        Some(total.parse::<u64>().ok()?)
    };
    Some((start, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_content_range() {
        assert_eq!(parse_content_range("bytes 100-199/500"), Some((100, Some(500))));
        assert_eq!(parse_content_range("bytes 100-199/*"), Some((100, None)));
        assert_eq!(parse_content_range("invalid"), None);
    }

    #[test]
    fn resume_requires_non_empty_part_file() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "axbit-resume-test-{}-{}",
            std::process::id(),
            suffix,
        ));
        fs::create_dir_all(&root).unwrap();

        let destination = root.join("kernel.bsp");
        let part_destination = root.join("kernel.bsp.part");
        let download = KernelDownload {
            relative_path: PathBuf::from("kernel.bsp"),
            destination,
            url: reqwest::Url::parse("https://example.com/kernel.bsp").unwrap(),
        };

        assert!(
            KernelDownloadResume::from_download(&download)
                .unwrap()
                .is_none()
        );
        fs::write(&part_destination, b"").unwrap();
        assert!(
            KernelDownloadResume::from_download(&download)
                .unwrap()
                .is_none()
        );
        fs::write(&part_destination, b"partial").unwrap();
        let resume = KernelDownloadResume::from_download(&download)
            .unwrap()
            .unwrap();
        assert_eq!(resume.part_destination, part_destination);
        assert_eq!(resume.downloaded_bytes(), 7);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_kernel_groups_from_config() {
        let groups = read_kernel_groups(
            &kernels_config(),
            &kernels_dir()
        ).unwrap();

        assert!(!groups.is_empty());
        assert!(groups.iter().any(|group| group.id == "base_kernels" && group.required));
        assert!(groups.iter().any(|group| group.id == "inner_solar_system" && group.required));
        assert!(groups.iter().all(|group| !group.kernels.is_empty()));
        assert!(groups.iter().flat_map(|group| &group.kernels).all(|kernel| {
            kernel.url.as_str().starts_with(NAIF_GENERIC_KERNELS_URL)
                && kernel.destination.ends_with(&kernel.relative_path)
        }));
    }

    #[test]
    fn reports_missing_empty_and_incomplete_files() {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("axbit-kernel-test-{}-{}", std::process::id(), suffix));
        let kernels = root.join("kernels");
        let config = root.join("kernels.toml");
        fs::create_dir_all(&root).unwrap();
        fs::write(&config, r#"
[base_kernels]
kernels = [{ file = "lsk/base.tls" }]

[groups.inner_solar_system]
name = "Inner Solar System"
kernels = [{ file = "spk/inner.bsp", time_bounds = ["2000-01-01", "2001-01-01"], ids = [[1, 10]] }]

[groups.optional]
name = "Optional"
kernels = [{ file = "spk/optional.bsp", time_bounds = ["2000-01-01", "2001-01-01"], ids = [[11, 20]] }]
"#).unwrap();

        let report = verify_kernel_dir_integrity(&config, &kernels).unwrap();
        assert_eq!(report.missing_required_files().len(), 2);
        assert!(!report.is_ready());

        fs::create_dir_all(kernels.join("lsk")).unwrap();
        fs::create_dir_all(kernels.join("spk")).unwrap();
        fs::write(kernels.join("lsk/base.tls"), b"base").unwrap();
        fs::write(kernels.join("spk/inner.bsp"), b"").unwrap();
        fs::write(kernels.join("spk/optional.bsp.part"), b"partial").unwrap();

        let report = verify_kernel_dir_integrity(&config, &kernels).unwrap();
        assert!(!report.is_ready());
        assert!(report.files.iter().any(|file| file.is_empty));
        assert_eq!(report.incomplete_files.len(), 1);

        fs::write(kernels.join("spk/inner.bsp"), b"inner").unwrap();
        let report = verify_kernel_dir_integrity(&config, &kernels).unwrap();
        assert!(report.is_ready());
        remove_incomplete_kernel_files(&report.incomplete_files).unwrap();
        assert!(!kernels.join("spk/optional.bsp.part").exists());
        fs::remove_dir_all(root).unwrap();
    }

    // #[test]
    // fn configured_kernels_have_remote_headers() {
    //     let client = create_client().unwrap();

    //     let groups = read_kernel_groups(
    //         &kernels_config(),
    //         &kernels_dir()
    //     ).unwrap();

    //     for kernel in groups.iter().flat_map(|group| &group.kernels) {
    //         let header = get_kernel_header(&client, kernel)
    //             .unwrap_or_else(|error| panic!("{}: {}", kernel.relative_path.display(), error));
    //         assert!(
    //             header.content_length.is_some_and(|bytes| bytes > 0),
    //             "{} has no Content-Length header",
    //             kernel.relative_path.display(),
    //         );
    //         assert!(
    //             header.last_modified.is_some(),
    //             "{} has no Last-Modified header",
    //             kernel.relative_path.display(),
    //         );
    //         assert_eq!(
    //             header.accept_ranges.as_deref(),
    //             Some("bytes"),
    //             "{} does not advertise byte ranges",
    //             kernel.relative_path.display(),
    //         );
    //     }
    // }
}
