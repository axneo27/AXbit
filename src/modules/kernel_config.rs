use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct KernelConfig {
    pub base_kernels: BaseKernelConfig,
    pub groups: HashMap<String, KernelGroupConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BaseKernelConfig {
    pub kernels: Vec<BaseKernelFileConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BaseKernelFileConfig {
    pub file: String,
    #[serde(default)]
    pub download_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct KernelGroupConfig {
    pub name: String,
    pub kernels: Vec<KernelFileConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct KernelFileConfig {
    pub file: String,
    #[serde(default)]
    pub download_url: Option<String>,
    pub time_bounds: [String; 2],
    pub ids: Vec<[i32; 2]>,
}
