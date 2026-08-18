/// hardware.rs
///
/// Collects a snapshot of the host machine's hardware relevant to model
/// recommendation and runtime safety. VRAM detection is NOT performed here;
/// it is added in Phase 1B when the recommendation engine is built.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use sysinfo::{Disks, System};

// ─── Public types ────────────────────────────────────────────────────────────

/// Snapshot of host hardware used by the recommendation engine and watchdog.
///
/// All size fields are in **gibibytes** (GiB, base-2) for consistency with
/// how `sysinfo` reports memory and how llama-server sizes are discussed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareReport {
    /// Human-readable CPU brand string (e.g. "Intel(R) Core(TM) i7-1260P").
    pub cpu_model: String,

    /// Logical CPU count (hardware threads, not physical cores).
    pub cpu_cores: usize,

    /// CPU architecture as reported by the Rust standard library
    /// (e.g. "x86_64", "aarch64").
    pub cpu_arch: String,

    /// Total physical RAM in GiB.
    pub ram_total_gb: f64,

    /// Operating system name (e.g. "Windows", "macOS", "Linux").
    pub os_name: String,

    /// Operating system version string (e.g. "11", "14.4", "22.04").
    pub os_version: String,

    /// Free disk space in GiB on the filesystem that contains
    /// `model_storage_path`.
    pub disk_free_gb: f64,

    /// The path where models will be stored. Used to determine which
    /// filesystem's free space is reported.
    pub model_storage_path: PathBuf,
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Collect a `HardwareReport` for the given model storage path.
///
/// `model_storage_path` does **not** need to exist yet; if it doesn't,
/// the function walks up the directory tree until it finds an ancestor that
/// is on a known disk, then reports free space for that disk. If no ancestor
/// matches any mounted disk, it falls back to the disk with the most free
/// space.
///
/// # Errors
///
/// Returns an error only if `sysinfo` fails to initialise — which in practice
/// never happens on supported platforms. All individual field failures
/// (unknown CPU brand, undetectable OS version, etc.) produce a safe default
/// string rather than an error.
pub fn collect(model_storage_path: PathBuf) -> Result<HardwareReport> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let cpu_model = cpu_brand(&sys);
    let cpu_cores = sys.cpus().len();
    let cpu_arch = std::env::consts::ARCH.to_string();

    let ram_total_gb = bytes_to_gib(sys.total_memory());

    let os_name = System::name().unwrap_or_else(|| "Unknown".to_string());
    let os_version = System::os_version().unwrap_or_else(|| "Unknown".to_string());

    let disks = Disks::new_with_refreshed_list();
    let disk_free_gb = bytes_to_gib(
        disk_free_bytes_for_path(&disks, &model_storage_path)
            .context("could not determine free disk space for model storage path")?,
    );

    Ok(HardwareReport {
        cpu_model,
        cpu_cores,
        cpu_arch,
        ram_total_gb,
        os_name,
        os_version,
        disk_free_gb,
        model_storage_path,
    })
}

// ─── Private helpers ─────────────────────────────────────────────────────────

/// Return the brand string of the first CPU, or a safe fallback.
fn cpu_brand(sys: &System) -> String {
    sys.cpus()
        .first()
        .map(|c| {
            let brand = c.brand().trim().to_string();
            if brand.is_empty() {
                "Unknown CPU".to_string()
            } else {
                brand
            }
        })
        .unwrap_or_else(|| "Unknown CPU".to_string())
}

/// Convert bytes (u64) to gibibytes (f64), rounded to two decimal places.
pub(crate) fn bytes_to_gib(bytes: u64) -> f64 {
    let gib = bytes as f64 / 1_073_741_824.0;
    (gib * 100.0).round() / 100.0
}

/// Find free bytes on the disk that best matches `path`.
///
/// Strategy:
/// 1. Walk `path` upward until an ancestor is a known mount point.
/// 2. If nothing matches, return the maximum free space across all disks.
/// 3. If there are no disks at all, return an error.
fn disk_free_bytes_for_path(disks: &Disks, path: &Path) -> Result<u64> {
    let disk_list: Vec<_> = disks.list().iter().collect();

    if disk_list.is_empty() {
        anyhow::bail!("sysinfo reported zero disks");
    }

    let mut candidate = Some(path);
    while let Some(p) = candidate {
        let mut best: Option<(usize, u64)> = None;
        for disk in &disk_list {
            let mount = disk.mount_point();
            if p == mount {
                let len = mount.as_os_str().len();
                if best.map_or(true, |(l, _)| len > l) {
                    best = Some((len, disk.available_space()));
                }
            }
        }
        if let Some((_, free)) = best {
            return Ok(free);
        }
        candidate = p.parent();
    }

    let max_free = disk_list
        .iter()
        .map(|d| d.available_space())
        .max()
        .unwrap_or(0);

    Ok(max_free)
}