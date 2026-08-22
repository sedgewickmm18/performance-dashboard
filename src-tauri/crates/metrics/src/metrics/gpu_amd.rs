use std::fs;
use std::path::{Path, PathBuf};
use serde::Serialize;

// ── Data types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    pub name: Option<String>,
    pub vendor: String,
    pub utilization_gpu: f32,
    pub utilization_memory: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub memory_free_mb: u64,
    pub temperature_c: f32,
    pub power_w: f32,
    pub core_clock_mhz: u64,
    pub memory_clock_mhz: u64,
    pub fan_speed_pct: f32,
}

// ── Low-level helpers ───────────────────────────────────────────────────────

fn read_sysfs_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_sysfs_u64(path: &Path) -> Option<u64> {
    read_sysfs_trim(path)?.parse::<u64>().ok()
}

/// Return the first `hwmon{N}` directory (N = 0–9) that exists under
/// `card_device_path/hwmon/`.
fn find_hwmon(card_device_path: &Path) -> Option<PathBuf> {
    for n in 0..10 {
        let p = card_device_path.join(format!("hwmon/hwmon{}", n));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

// ── AMD card detection ──────────────────────────────────────────────────────

/// Find the first card path whose `device/vendor` file contains `"0x1002"`.
/// `drm_root` is normally `/sys/class/drm`.
fn find_amd_card(drm_root: &Path) -> Option<PathBuf> {
    for n in 0..10 {
        let card = drm_root.join(format!("card{}", n));
        let vendor_path = card.join("device/vendor");
        if let Some(v) = read_sysfs_trim(&vendor_path) {
            if v == "0x1002" {
                return Some(card);
            }
        }
    }
    None
}

// ── pp_dpm clock parsing ────────────────────────────────────────────────────

/// Parse a `pp_dpm_sclk` / `pp_dpm_mclk` file and return the MHz value on
/// the line that ends with `*`.
///
/// Example content:
/// ```text
/// 0: 500Mhz
/// 1: 1800Mhz *
/// 2: 2200Mhz
/// ```
fn parse_pp_dpm(content: &str) -> Option<u64> {
    for line in content.lines() {
        if line.contains('*') {
            // Line looks like: "1: 1800Mhz *"
            // Find the number immediately before "Mhz"
            if let Some(mhz_pos) = line.to_lowercase().find("mhz") {
                let before = &line[..mhz_pos];
                // Walk backwards past the digits
                let digits: String = before.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    let rev: String = digits.chars().rev().collect();
                    return rev.parse::<u64>().ok();
                }
            }
        }
    }
    None
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Read all AMD GPU stats starting from the real sysfs root `/sys/class/drm`.
pub fn read_gpu_stats() -> Option<GpuStats> {
    read_gpu_stats_from(Path::new("/sys/class/drm"))
}

/// Same as [`read_gpu_stats`] but accepts an arbitrary DRM root so tests can
/// pass a `TempDir`-backed path instead of `/sys/class/drm`.
pub fn read_gpu_stats_from(drm_root: &Path) -> Option<GpuStats> {
    let card_path = find_amd_card(drm_root)?;
    let device_path = card_path.join("device");
    let hwmon = find_hwmon(&device_path);

    // GPU utilisation
    let utilization_gpu = round1(
        read_sysfs_u64(&device_path.join("gpu_busy_percent"))
            .unwrap_or(0) as f32,
    );

    // VRAM
    let memory_total_mb = read_sysfs_u64(&device_path.join("mem_info_vram_total"))
        .unwrap_or(0) / 1_048_576;
    let memory_used_mb = read_sysfs_u64(&device_path.join("mem_info_vram_used"))
        .unwrap_or(0) / 1_048_576;
    let memory_free_mb = memory_total_mb.saturating_sub(memory_used_mb);
    let utilization_memory = if memory_total_mb > 0 {
        round1(memory_used_mb as f32 / memory_total_mb as f32 * 100.0)
    } else {
        0.0
    };

    // Temperature — millidegrees → °C
    let temperature_c = hwmon
        .as_ref()
        .and_then(|h| read_sysfs_u64(&h.join("temp1_input")))
        .map(|v| round1(v as f32 / 1_000.0))
        .unwrap_or(0.0);

    // Power — microwatts → W
    let power_w = hwmon
        .as_ref()
        .and_then(|h| read_sysfs_u64(&h.join("power1_average")))
        .map(|v| round1(v as f32 / 1_000_000.0))
        .unwrap_or(0.0);

    // Fan — PWM 0-255 → 0-100 %
    let fan_speed_pct = hwmon
        .as_ref()
        .and_then(|h| read_sysfs_u64(&h.join("pwm1")))
        .map(|v| round1(v as f32 / 255.0 * 100.0))
        .unwrap_or(0.0);

    // Core clock
    let core_clock_mhz = read_sysfs_trim(&device_path.join("pp_dpm_sclk"))
        .as_deref()
        .and_then(parse_pp_dpm)
        .unwrap_or(0);

    // Memory clock
    let memory_clock_mhz = read_sysfs_trim(&device_path.join("pp_dpm_mclk"))
        .as_deref()
        .and_then(parse_pp_dpm)
        .unwrap_or(0);

    // GPU name — prefer product_name, fallback to model
    let name = read_sysfs_trim(&device_path.join("product_name"))
        .or_else(|| read_sysfs_trim(&device_path.join("model")));

    Some(GpuStats {
        name,
        vendor: "AMD".to_string(),
        utilization_gpu,
        utilization_memory,
        memory_used_mb,
        memory_total_mb,
        memory_free_mb,
        temperature_c,
        power_w,
        core_clock_mhz,
        memory_clock_mhz,
        fan_speed_pct,
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a minimal fake sysfs tree under `tmp` and return the DRM root path.
    ///
    /// Tree layout:
    /// ```
    /// {tmp}/sys/class/drm/card0/device/vendor
    /// {tmp}/sys/class/drm/card0/device/gpu_busy_percent
    /// {tmp}/sys/class/drm/card0/device/mem_info_vram_total
    /// {tmp}/sys/class/drm/card0/device/mem_info_vram_used
    /// {tmp}/sys/class/drm/card0/device/product_name
    /// {tmp}/sys/class/drm/card0/device/hwmon/hwmon1/temp1_input
    /// {tmp}/sys/class/drm/card0/device/hwmon/hwmon1/power1_average
    /// {tmp}/sys/class/drm/card0/device/hwmon/hwmon1/pwm1
    /// {tmp}/sys/class/drm/card0/device/pp_dpm_sclk
    /// {tmp}/sys/class/drm/card0/device/pp_dpm_mclk
    /// ```
    fn build_fake_sysfs(tmp: &TempDir) -> PathBuf {
        let drm_root = tmp.path().join("sys/class/drm");
        let device = drm_root.join("card0/device");
        let hwmon = device.join("hwmon/hwmon1");

        fs::create_dir_all(&device).unwrap();
        fs::create_dir_all(&hwmon).unwrap();

        // Vendor — AMD  (find_amd_card looks for card0/device/vendor)
        fs::write(device.join("vendor"), "0x1002\n").unwrap();
        // Utilisation
        fs::write(device.join("gpu_busy_percent"), "72\n").unwrap();
        // VRAM  (8 GiB total, 2 GiB used)
        fs::write(device.join("mem_info_vram_total"), "8589934592\n").unwrap();
        fs::write(device.join("mem_info_vram_used"), "2147483648\n").unwrap();
        // Name
        fs::write(device.join("product_name"), "Radeon RX 7900 XTX\n").unwrap();
        // Temperature  65 000 millidegrees = 65 °C
        fs::write(hwmon.join("temp1_input"), "65000\n").unwrap();
        // Power  120 000 000 µW = 120 W
        fs::write(hwmon.join("power1_average"), "120000000\n").unwrap();
        // Fan PWM  178 → 178/255*100 ≈ 69.8 %
        fs::write(hwmon.join("pwm1"), "178\n").unwrap();
        // Core clock
        fs::write(
            device.join("pp_dpm_sclk"),
            "0: 500Mhz\n1: 1800Mhz *\n2: 2200Mhz\n",
        )
        .unwrap();
        // Memory clock
        fs::write(
            device.join("pp_dpm_mclk"),
            "0: 96Mhz\n1: 1000Mhz *\n",
        )
        .unwrap();

        drm_root
    }

    #[test]
    fn test_gpu_utilisation() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).expect("should find AMD card");
        assert_eq!(stats.utilization_gpu, 72.0);
    }

    #[test]
    fn test_vram_fields() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).unwrap();
        assert_eq!(stats.memory_total_mb, 8192);
        assert_eq!(stats.memory_used_mb, 2048);
        assert_eq!(stats.memory_free_mb, 6144);
    }

    #[test]
    fn test_memory_utilisation() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).unwrap();
        assert_eq!(stats.utilization_memory, 25.0);
    }

    #[test]
    fn test_temperature() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).unwrap();
        assert_eq!(stats.temperature_c, 65.0);
    }

    #[test]
    fn test_power() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).unwrap();
        assert_eq!(stats.power_w, 120.0);
    }

    #[test]
    fn test_clocks() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).unwrap();
        assert_eq!(stats.core_clock_mhz, 1800);
        assert_eq!(stats.memory_clock_mhz, 1000);
    }

    #[test]
    fn test_fan_speed() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).unwrap();
        // 178 / 255 * 100 = 69.803… → rounds to 69.8
        assert_eq!(stats.fan_speed_pct, 69.8);
    }

    #[test]
    fn test_gpu_name_and_vendor() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read_gpu_stats_from(&drm).unwrap();
        assert_eq!(stats.name, Some("Radeon RX 7900 XTX".to_string()));
        assert_eq!(stats.vendor, "AMD");
    }

    #[test]
    fn test_non_amd_vendor_returns_none() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        // Overwrite vendor with Intel PCI ID
        let vendor_path = drm.join("card0/device/vendor");
        fs::write(&vendor_path, "0x8086\n").unwrap();
        assert!(read_gpu_stats_from(&drm).is_none());
    }
}
