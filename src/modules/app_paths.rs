use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};

static APP_PATHS: OnceLock<AppPaths> = OnceLock::new();

#[derive(Debug)]
pub struct AppPaths {
    settings: PathBuf,
    sbdb: PathBuf,
    kernels: PathBuf,
    kernel_manifest: PathBuf,
    resources: PathBuf,
}

impl AppPaths {
    fn discover() -> Result<Self> {
        #[cfg(feature = "packaged")]
        {
            use cargo_packager_resource_resolver::{current_format, resources_dir};
            use directories::ProjectDirs;

            let project_dirs = ProjectDirs::from("com", "axneo27", "AXbit")
                .context("could not determine AXbit application directories")?;
            let resources = resources_dir(current_format()?)
                .context("could not determine packaged resource directory")?;

            Ok(Self {
                settings: project_dirs.config_dir().join("settings.toml"),
                sbdb: project_dirs.data_local_dir().join("sbdb.db"),
                kernels: project_dirs.data_local_dir().join("kernels"),
                kernel_manifest: resources.join("kernels.toml"),
                resources: resources.join("res"),
            })
        }

        #[cfg(not(feature = "packaged"))]
        {
            let root = std::env::current_dir().context("could not determine working directory")?;
            let paths = Self {
                settings: root.join("config/settings.toml"),
                sbdb: root.join("data/sbdb.db"),
                kernels: root.join("spice-tools/kernels"),
                kernel_manifest: root.join("kernels.toml"),
                resources: PathBuf::from(env!("OUT_DIR")).join("res"),
            };
            paths.migrate_development_settings()?;
            Ok(paths)
        }
    }

    #[cfg(not(feature = "packaged"))]
    fn migrate_development_settings(&self) -> Result<()> {
        let old_path = self
            .sbdb
            .parent()
            .unwrap_or(Path::new("."))
            .join("settings.toml");
        if self.settings.exists() || !old_path.exists() {
            return Ok(());
        }
        if let Some(parent) = self.settings.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::copy(&old_path, &self.settings).with_context(|| {
            format!(
                "failed to migrate settings from {} to {}",
                old_path.display(),
                self.settings.display()
            )
        })?;
        Ok(())
    }

    pub fn settings(&self) -> &Path {
        &self.settings
    }
    pub fn sbdb(&self) -> &Path {
        &self.sbdb
    }
    pub fn kernels(&self) -> &Path {
        &self.kernels
    }
    pub fn kernel_manifest(&self) -> &Path {
        &self.kernel_manifest
    }
    pub fn resources(&self) -> &Path {
        &self.resources
    }
}

pub fn init() -> Result<&'static AppPaths> {
    if let Some(paths) = APP_PATHS.get() {
        return Ok(paths);
    }
    let paths = AppPaths::discover()?;
    let _ = APP_PATHS.set(paths);
    Ok(APP_PATHS.get().expect("application paths were initialized"))
}

pub fn get() -> &'static AppPaths {
    init().expect("failed to initialize application paths")
}
