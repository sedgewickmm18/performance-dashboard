use std::collections::HashSet;
use std::fs;
use std::path::Path;
use serde::Serialize;

use crate::metrics::gpu_amd;

// ── Data types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuStaticInfo {
    pub manufacturer: String,
    pub brand: String,
    pub speed: f64,
    pub speed_max: f64,
    pub cores: usize,
    pub physical_cores: usize,
    pub socket: usize,
    pub cache: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    pub platform: String,
    pub distro: String,
    pub release: String,
    pub arch: String,
    pub hostname: String,
    pub kernel: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStaticInfo {
    pub model: Option<String>,
    pub vendor: String,
    pub vram: u64,
    pub vram_dynamic: bool,
    pub driver_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiskInfo {
    pub fs: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub size: u64,
    pub used: u64,
    pub available: u64,
    #[serde(rename = "use")]
    pub use_percent: f32,
    pub mount: String,
}

#[derive(Debug, Serialize)]
pub struct StaticInfo {
    pub cpu: CpuStaticInfo,
    pub os: OsInfo,
    pub gpu: Vec<GpuStaticInfo>,
    pub disks: Vec<DiskInfo>,
}

// ── CPU parsing ─────────────────────────────────────────────────────────────

/// Parse `/proc/cpuinfo` content and return a `CpuStaticInfo`.
pub fn parse_cpuinfo(content: &str) -> CpuStaticInfo {
    let mut brand = String::new();
    let mut speed: f64 = 0.0;
    let mut processor_count: usize = 0;
    let mut core_ids: HashSet<String> = HashSet::new();
    let mut physical_ids: HashSet<String> = HashSet::new();
    let mut cache = String::new();

    for line in content.lines() {
        if let Some((key, value)) = split_cpuinfo_line(line) {
            match key {
                "model name" => {
                    if brand.is_empty() {
                        brand = value.to_string();
                    }
                }
                "cpu MHz" => {
                    if speed == 0.0 {
                        speed = value.trim().parse::<f64>().unwrap_or(0.0);
                    }
                }
                "processor" => {
                    processor_count += 1;
                }
                "core id" => {
                    core_ids.insert(value.trim().to_string());
                }
                "physical id" => {
                    physical_ids.insert(value.trim().to_string());
                }
                "cache size" => {
                    if cache.is_empty() {
                        cache = value.trim().to_string();
                    }
                }
                _ => {}
            }
        }
    }

    // Manufacturer: first word before '(' or whitespace
    let manufacturer = brand
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_string();

    let physical_cores = if core_ids.is_empty() { processor_count } else { core_ids.len() };
    let socket = if physical_ids.is_empty() { 1 } else { physical_ids.len() };

    CpuStaticInfo {
        manufacturer,
        brand,
        speed,
        speed_max: speed,
        cores: processor_count,
        physical_cores,
        socket,
        cache,
    }
}

fn split_cpuinfo_line(line: &str) -> Option<(&str, &str)> {
    let pos = line.find(':')?;
    let key = line[..pos].trim();
    let value = line[pos + 1..].trim();
    Some((key, value))
}

// ── OS info ─────────────────────────────────────────────────────────────────

/// Parse `/etc/os-release` content into `(distro, release)`.
pub fn parse_os_release(content: &str) -> (String, String) {
    let mut distro = String::new();
    let mut release = String::new();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("NAME=") {
            distro = rest.trim_matches('"').to_string();
        } else if let Some(rest) = line.strip_prefix("VERSION_ID=") {
            release = rest.trim_matches('"').to_string();
        }
    }
    (distro, release)
}

fn read_os_info() -> OsInfo {
    read_os_info_from(
        Path::new("/etc/os-release"),
        Path::new("/proc/version"),
        Path::new("/proc/sys/kernel/hostname"),
    )
}

pub fn read_os_info_from(
    os_release_path: &Path,
    proc_version_path: &Path,
    hostname_path: &Path,
) -> OsInfo {
    let os_release = fs::read_to_string(os_release_path).unwrap_or_default();
    let (distro, release) = parse_os_release(&os_release);

    // Kernel version: second word of /proc/version
    let proc_version = fs::read_to_string(proc_version_path).unwrap_or_default();
    let kernel = proc_version
        .split_whitespace()
        .nth(2)
        .unwrap_or("")
        .to_string();

    let hostname = fs::read_to_string(hostname_path)
        .unwrap_or_default()
        .trim()
        .to_string();

    OsInfo {
        platform: "linux".to_string(),
        distro,
        release,
        arch: std::env::consts::ARCH.to_string(),
        hostname,
        kernel,
    }
}

// ── GPU static info ─────────────────────────────────────────────────────────

fn read_gpu_static() -> Vec<GpuStaticInfo> {
    match gpu_amd::read_gpu_stats() {
        Some(stats) => {
            let driver_version = fs::read_to_string(
                "/sys/class/drm/card0/device/driver/module/version",
            )
            .ok()
            .map(|s| s.trim().to_string());
            vec![GpuStaticInfo {
                model: stats.name,
                vendor: stats.vendor,
                vram: stats.memory_total_mb,
                vram_dynamic: false,
                driver_version,
            }]
        }
        None => vec![],
    }
}

// ── Disk info ────────────────────────────────────────────────────────────────

const PSEUDO_FS: &[&str] = &[
    "proc", "sysfs", "devtmpfs", "tmpfs", "devpts", "cgroup", "cgroup2",
    "pstore", "bpf", "tracefs", "hugetlbfs", "mqueue", "debugfs",
    "securityfs", "fusectl", "configfs", "efivarfs", "autofs",
];

/// Parse `/proc/mounts` lines into `(device, mount_point, fs_type)` triples,
/// filtering out pseudo-filesystems.
pub fn parse_mounts(content: &str) -> Vec<(String, String, String)> {
    let mut result = Vec::new();
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let device = match parts.next() { Some(s) => s, None => continue };
        let mount  = match parts.next() { Some(s) => s, None => continue };
        let fstype = match parts.next() { Some(s) => s, None => continue };
        if PSEUDO_FS.contains(&fstype) {
            continue;
        }
        result.push((device.to_string(), mount.to_string(), fstype.to_string()));
    }
    result
}

fn statvfs_disk(mount: &str) -> Option<(u64, u64, u64)> {
    use std::ffi::CString;
    let c_path = CString::new(mount).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
    if ret != 0 {
        return None;
    }
    let bsize = st.f_bsize as u64;
    let size      = st.f_blocks * bsize;
    let available = st.f_bavail  * bsize;
    let used      = size.saturating_sub(st.f_bfree * bsize);
    Some((size, used, available))
}

fn read_disks() -> Vec<DiskInfo> {
    read_disks_from(Path::new("/proc/mounts"))
}

pub fn read_disks_from(mounts_path: &Path) -> Vec<DiskInfo> {
    let content = match fs::read_to_string(mounts_path) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let mut seen = HashSet::new();
    let mut disks = Vec::new();
    for (device, mount, fstype) in parse_mounts(&content) {
        if !seen.insert(device.clone()) {
            continue;
        }
        if let Some((size, used, available)) = statvfs_disk(&mount) {
            let use_percent = if size > 0 {
                let pct = used as f32 / size as f32 * 100.0;
                (pct * 10.0).round() / 10.0
            } else {
                0.0
            };
            disks.push(DiskInfo {
                fs: device,
                type_: fstype,
                size,
                used,
                available,
                use_percent,
                mount,
            });
        }
    }
    disks
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn read_static_info() -> StaticInfo {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = parse_cpuinfo(&cpuinfo);
    let os = read_os_info();
    let gpu = read_gpu_static();
    let disks = read_disks();
    StaticInfo { cpu, os, gpu, disks }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const CPUINFO_2CORE: &str = "\
processor\t: 0\nvendor_id\t: GenuineIntel\ncpu family\t: 6\nmodel\t: 142\n\
model name\t: Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz\nstepping\t: 10\n\
cpu MHz\t\t: 1992.000\ncache size\t: 8192 KB\nphysical id\t: 0\ncore id\t: 0\n\n\
processor\t: 1\nvendor_id\t: GenuineIntel\ncpu family\t: 6\nmodel\t: 142\n\
model name\t: Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz\nstepping\t: 10\n\
cpu MHz\t\t: 1992.000\ncache size\t: 8192 KB\nphysical id\t: 0\ncore id\t: 1\n\n";

    #[test]
    fn test_cpuinfo_manufacturer() {
        let info = parse_cpuinfo(CPUINFO_2CORE);
        assert_eq!(info.manufacturer, "Intel");
    }

    #[test]
    fn test_cpuinfo_brand() {
        let info = parse_cpuinfo(CPUINFO_2CORE);
        assert_eq!(info.brand, "Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz");
    }

    #[test]
    fn test_cpuinfo_cores() {
        let info = parse_cpuinfo(CPUINFO_2CORE);
        assert_eq!(info.cores, 2);
    }

    #[test]
    fn test_cpuinfo_physical_cores() {
        let info = parse_cpuinfo(CPUINFO_2CORE);
        assert_eq!(info.physical_cores, 2); // two distinct core id values: 0 and 1
    }

    #[test]
    fn test_cpuinfo_amd_manufacturer() {
        let content = "processor\t: 0\nmodel name\t: AMD Ryzen 9 5900X\ncpu MHz\t: 3700.0\n";
        let info = parse_cpuinfo(content);
        assert_eq!(info.manufacturer, "AMD");
    }

    #[test]
    fn test_os_release_parsing() {
        let tmp = TempDir::new().unwrap();
        let os_release = tmp.path().join("os-release");
        let proc_ver   = tmp.path().join("version");
        let hostname_f = tmp.path().join("hostname");

        fs::write(&os_release, "NAME=\"Fedora Linux\"\nVERSION_ID=\"39\"\n").unwrap();
        fs::write(&proc_ver,   "Linux version 6.7.0-rc5 (gcc version ...)\n").unwrap();
        fs::write(&hostname_f, "myhost\n").unwrap();

        let os = read_os_info_from(&os_release, &proc_ver, &hostname_f);
        assert_eq!(os.platform, "linux");
        assert_eq!(os.distro,   "Fedora Linux");
        assert_eq!(os.release,  "39");
        assert_eq!(os.kernel,   "6.7.0-rc5");
        assert_eq!(os.hostname, "myhost");
    }

    #[test]
    fn test_mounts_pseudo_filtered() {
        let content = "\
sysfs /sys sysfs rw 0 0\n\
tmpfs /run tmpfs rw 0 0\n\
/dev/sda1 / ext4 rw 0 0\n\
/dev/sda2 /home ext4 rw 0 0\n\
proc /proc proc rw 0 0\n";
        let mounts = parse_mounts(content);
        // Only real block devices survive
        let mount_points: Vec<&str> = mounts.iter().map(|(_, m, _)| m.as_str()).collect();
        assert!(!mount_points.contains(&"/sys"));
        assert!(!mount_points.contains(&"/run"));
        assert!(!mount_points.contains(&"/proc"));
        assert!(mount_points.contains(&"/"));
        assert!(mount_points.contains(&"/home"));
    }
}
