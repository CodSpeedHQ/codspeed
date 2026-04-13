use crate::executor::helpers::apt;
use crate::executor::wall_time::perf::perf_executable::get_working_perf_executable;
use crate::executor::{ToolInstallStatus, ToolStatus};
use crate::prelude::*;
use crate::system::SystemInfo;

use std::{path::Path, process::Command};

const TOOL_NAME: &str = "perf";

pub fn get_perf_status() -> ToolStatus {
    let tool_name = TOOL_NAME.to_string();
    match get_working_perf_executable() {
        Some(perf_path) => {
            let version = Command::new(&perf_path)
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            ToolStatus {
                tool_name,
                status: ToolInstallStatus::Installed { version },
            }
        }
        None => ToolStatus {
            tool_name,
            status: ToolInstallStatus::NotInstalled,
        },
    }
}

fn is_perf_installed() -> bool {
    get_working_perf_executable().is_some()
}

pub async fn install_perf(system_info: &SystemInfo, setup_cache_dir: Option<&Path>) -> Result<()> {
    apt::install_cached(system_info, setup_cache_dir, is_perf_installed, || async {
        debug!("Installing perf");
        let cmd = Command::new("uname")
            .arg("-r")
            .output()
            .expect("Failed to execute uname");
        let kernel_release = String::from_utf8_lossy(&cmd.stdout);
        let kernel_release = kernel_release.trim();
        let linux_tools_kernel_release = format!("linux-tools-{kernel_release}");

        let packages = vec![
            "linux-tools-common".to_string(),
            "linux-tools-generic".to_string(),
            linux_tools_kernel_release,
        ];
        let package_refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();

        apt::install(system_info, &package_refs)?;

        // Return package names for caching
        Ok(packages)
    })
    .await
}
