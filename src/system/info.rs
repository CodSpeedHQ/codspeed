use std::process::Command;

use serde::Serialize;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::prelude::*;
use crate::system::os::SupportedOs;

fn get_user() -> Result<String> {
    let user_output = Command::new("whoami")
        .output()
        .map_err(|_| anyhow!("Failed to get user info"))?;
    if !user_output.status.success() {
        bail!("Failed to get user info");
    }
    let output_str =
        String::from_utf8(user_output.stdout).map_err(|_| anyhow!("Failed to parse user info"))?;
    Ok(output_str.trim().to_string())
}

#[derive(Eq, PartialEq, Hash, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    /// Flattened to the `os` and `osVersion` fields on the wire via [`SupportedOs`]'s serde impl.
    #[serde(flatten)]
    pub os: SupportedOs,
    pub arch: String,
    pub host: String,
    pub user: String,
    pub cpu_brand: String,
    pub cpu_name: String,
    pub cpu_vendor_id: String,
    pub cpu_cores: usize,
    pub total_memory_gb: u64,
    pub cpu_flags: Vec<String>,
}

#[cfg(test)]
impl SystemInfo {
    pub fn test() -> Self {
        SystemInfo {
            os: SupportedOs::Linux(crate::system::LinuxDistribution::Ubuntu {
                version: "20.04".into(),
            }),
            arch: "x86_64".to_string(),
            host: "host".to_string(),
            user: "user".to_string(),
            cpu_brand: "Intel(R) Xeon(R) CPU E5-2686 v4 @ 2.30GHz".to_string(),
            cpu_name: "cpu0".to_string(),
            cpu_vendor_id: "GenuineIntel".to_string(),
            cpu_cores: 2,
            total_memory_gb: 8,
            cpu_flags: vec![
                "sse2".to_string(),
                "avx".to_string(),
                "avx2".to_string(),
                "erms".to_string(),
            ],
        }
    }
}

#[cfg(target_os = "linux")]
fn get_cpu_flags() -> Vec<String> {
    use procfs::Current;

    let cpuinfo = match procfs::CpuInfo::current() {
        Ok(cpuinfo) => cpuinfo,
        Err(e) => {
            warn!("Failed to read /proc/cpuinfo: {e}");
            return Vec::new();
        }
    };

    // /proc/cpuinfo uses "flags" on x86_64 and "Features" on aarch64
    let field_name = if cfg!(target_arch = "x86_64") {
        "flags"
    } else if cfg!(target_arch = "aarch64") {
        "Features"
    } else {
        return Vec::new();
    };

    let mut flags: Vec<String> = match cpuinfo.get_field(0, field_name) {
        Some(value) => value.split_whitespace().map(|s| s.to_string()).collect(),
        None => {
            warn!("No CPU flags found in /proc/cpuinfo (field: {field_name})");
            return Vec::new();
        }
    };
    flags.sort();
    flags
}

#[cfg(not(target_os = "linux"))]
fn get_cpu_flags() -> Vec<String> {
    Vec::new()
}

impl SystemInfo {
    pub fn new() -> Result<Self> {
        let os = SupportedOs::from_os(std::env::consts::OS)?;
        let arch = System::cpu_arch();
        let user = get_user()?;
        let host = System::host_name().ok_or(anyhow!("Failed to get host name"))?;

        let s = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        let cpu_cores = s
            .physical_core_count()
            .ok_or(anyhow!("Failed to get CPU core count"))?;
        let total_memory_gb = s.total_memory().div_ceil(1024_u64.pow(3));

        // take the first CPU to get the brand, name and vendor id
        let cpu = s
            .cpus()
            .iter()
            .next()
            .ok_or(anyhow!("Failed to get CPU info"))?;
        let cpu_brand = {
            let brand = cpu.brand().to_string();
            if brand.is_empty() {
                "unknown".to_string()
            } else {
                brand
            }
        };
        let cpu_name = cpu.name().to_string();
        let cpu_vendor_id = cpu.vendor_id().to_string();

        let cpu_flags = get_cpu_flags();

        Ok(SystemInfo {
            os,
            arch,
            host,
            user,
            cpu_brand,
            cpu_name,
            cpu_vendor_id,
            cpu_cores,
            total_memory_gb,
            cpu_flags,
        })
    }
}
