use std::fs;
use std::path::{Path, PathBuf};
use serde::Serialize;

// ── Data types ─────────────────────────────────────────────────────────────
//
// Sensor modelled after amdgpu_top (crates/libamdgpu_top/src/stat/sensors.rs):
// edge/junction/memory temperature, average/input power + power cap, fan RPM,
// vddgfx/vddnb voltage, sclk/mclk and the PCIe link — all read from the
// amdgpu kernel interfaces under /sys/class/drm/card*/device.

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    // Identity
    pub name: Option<String>,
    pub vendor: String,
    /// PCI slot address, e.g. "0000:c5:00.0" (uevent → PCI_SLOT_NAME)
    pub pci_slot: Option<String>,
    /// PCI vendor:device ID, e.g. "1002:1586"
    pub pci_device_id: Option<String>,
    /// amdgpu `unique_id` sysfs attribute (empty on many ASICs)
    pub unique_id: Option<String>,

    // Utilisation — f64 so the rounded values survive the f32→f64 widening
    // in serde_json::Value (f32 17.6 widened prints as 17.600000381469727)
    pub utilization_gpu: f64,
    pub utilization_memory: f64,

    // VRAM (dedicated) + GTT (host-memory-backed DMA buffer pool)
    #[serde(rename = "memoryUsedMB")]
    pub memory_used_mb: u64,
    #[serde(rename = "memoryTotalMB")]
    pub memory_total_mb: u64,
    #[serde(rename = "memoryFreeMB")]
    pub memory_free_mb: u64,
    #[serde(rename = "gttUsedMB")]
    pub gtt_used_mb: Option<u64>,
    #[serde(rename = "gttTotalMB")]
    pub gtt_total_mb: Option<u64>,

    // Temperatures — hwmon temp1/2/3 map to edge/junction/memory
    pub temperature_c: f64,
    pub temperature_junction_c: Option<f64>,
    pub temperature_memory_c: Option<f64>,

    // Power — power1_average, falling back to power1_input (amdgpu_top
    // uses whichever the ASIC exposes)
    pub power_w: f64,
    pub power_cap_w: Option<f64>,
    pub power_cap_max_w: Option<f64>,

    // Clocks — freq1_input (Hz) preferred, pp_dpm_* as fallback
    #[serde(rename = "coreClockMHz")]
    pub core_clock_mhz: u64,
    #[serde(rename = "memoryClockMHz")]
    pub memory_clock_mhz: u64,

    // Voltages (hwmon in0/in1, millivolts)
    pub vddgfx_mv: Option<u64>,
    pub vddnb_mv: Option<u64>,

    // Fan — pwm1 as 0-100 %, fan1_input/fan1_max as RPM
    pub fan_speed_pct: f64,
    pub fan_rpm: Option<u64>,
    pub fan_max_rpm: Option<u64>,

    // PCIe link — current_link_{speed,width} / max_link_{speed,width}
    pub pcie_gen: Option<u8>,
    pub pcie_width: Option<u32>,
    pub pcie_max_gen: Option<u8>,
    pub pcie_max_width: Option<u32>,
}

// ── Low-level helpers ───────────────────────────────────────────────────────

/// Read a sysfs attribute and trim it. Returns `None` for missing files and
/// for empty contents (e.g. `product_name` is an empty file on Strix Halo).
fn read_sysfs_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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

/// Parse `KEY=value` pairs from a sysfs `uevent` file.
fn read_uevent_field(device_path: &Path, key: &str) -> Option<String> {
    let uevent = read_sysfs_trim(&device_path.join("uevent"))?;
    for line in uevent.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k == key {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Map a PCIe link speed like `"16.0 GT/s PCIe"` to its generation number.
fn pcie_gen_from_speed(speed: &str) -> Option<u8> {
    let gt_s: f32 = speed.split_whitespace().next()?.parse().ok()?;
    match gt_s {
        s if s <= 2.5 => Some(1),
        s if s <= 5.0 => Some(2),
        s if s <= 8.0 => Some(3),
        s if s <= 16.0 => Some(4),
        s if s <= 32.0 => Some(5),
        s if s <= 64.0 => Some(6),
        _ => None,
    }
}

/// Look up the marketing name for `vendor:device` in a hwdata-style
/// `pci.ids` file. Returns e.g. `"Strix Halo [Radeon Graphics / …]"` for
/// `1002:1586`, mirroring the PCI ID table amdgpu_top carries in
/// libdrm_amdgpu_sys — but resolved from the system database at runtime
/// so no table has to be vendored.
fn lookup_hwdata_name(pci_ids: &Path, vendor: &str, device: &str) -> Option<String> {
    let content = fs::read_to_string(pci_ids).ok()?;
    let mut in_vendor = false;
    for line in content.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with('\t') {
            // Top-level vendor line: "1002  Advanced Micro Devices, Inc. [AMD/ATI]"
            in_vendor = line.split_whitespace().next() == Some(vendor);
        } else if in_vendor {
            // Device line: "\t1586  Strix Halo [Radeon Graphics / …]"
            let entry = line.trim_start();
            let mut parts = entry.splitn(2, "  ");
            if parts.next() == Some(device) {
                return parts.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            }
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

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// hwmon temperature in millidegrees → °C
fn hwmon_temp_c(hwmon: &Option<PathBuf>, n: u8) -> Option<f64> {
    hwmon
        .as_ref()
        .and_then(|h| read_sysfs_u64(&h.join(format!("temp{}_input", n))))
        .map(|v| round1(v as f64 / 1_000.0))
}

// ── Public API ──────────────────────────────────────────────────────────────

/// System `pci.ids` databases, in hwdata (Fedora/RHEL) then Debian order.
const PCI_IDS_PATHS: &[&str] = &[
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
];

/// Read all AMD GPU stats starting from the real sysfs root `/sys/class/drm`.
pub fn read_gpu_stats() -> Option<GpuStats> {
    let pci_ids: Vec<PathBuf> = PCI_IDS_PATHS.iter().map(PathBuf::from).collect();
    read_gpu_stats_with(Path::new("/sys/class/drm"), &pci_ids)
}

/// Same as [`read_gpu_stats`] but accepts an arbitrary DRM root and `pci.ids`
/// database paths so tests can pass `TempDir`-backed paths instead of the
/// real `/sys/class/drm`.
pub fn read_gpu_stats_with(drm_root: &Path, pci_ids_paths: &[PathBuf]) -> Option<GpuStats> {
    let card_path = find_amd_card(drm_root)?;
    let device_path = card_path.join("device");
    let hwmon = find_hwmon(&device_path);

    // GPU utilisation
    let utilization_gpu = round1(
        read_sysfs_u64(&device_path.join("gpu_busy_percent"))
            .unwrap_or(0) as f64,
    );

    // VRAM
    let memory_total_mb = read_sysfs_u64(&device_path.join("mem_info_vram_total"))
        .unwrap_or(0) / 1_048_576;
    let memory_used_mb = read_sysfs_u64(&device_path.join("mem_info_vram_used"))
        .unwrap_or(0) / 1_048_576;
    let memory_free_mb = memory_total_mb.saturating_sub(memory_used_mb);
    let utilization_memory = if memory_total_mb > 0 {
        round1(memory_used_mb as f64 / memory_total_mb as f64 * 100.0)
    } else {
        0.0
    };

    // GTT
    let gtt_used_mb = read_sysfs_u64(&device_path.join("mem_info_gtt_used"))
        .map(|b| b / 1_048_576);
    let gtt_total_mb = read_sysfs_u64(&device_path.join("mem_info_gtt_total"))
        .map(|b| b / 1_048_576);

    // Temperatures — millidegrees → °C
    let temperature_c = hwmon_temp_c(&hwmon, 1).unwrap_or(0.0);
    let temperature_junction_c = hwmon_temp_c(&hwmon, 2);
    let temperature_memory_c = hwmon_temp_c(&hwmon, 3);

    // Power — microwatts → W. amdgpu_top prefers power1_average and falls
    // back to power1_input on ASICs that only expose the latter.
    let read_power = |file: &str| -> Option<f64> {
        hwmon
            .as_ref()
            .and_then(|h| read_sysfs_u64(&h.join(file)))
            .map(|v| round1(v as f64 / 1_000_000.0))
    };
    let power_w = read_power("power1_average")
        .or_else(|| read_power("power1_input"))
        .unwrap_or(0.0);
    let power_cap_w = read_power("power1_cap");
    let power_cap_max_w = read_power("power1_cap_max");

    // Core clock — live freq1_input (Hz) preferred, pp_dpm_sclk fallback
    let core_clock_mhz = hwmon
        .as_ref()
        .and_then(|h| read_sysfs_u64(&h.join("freq1_input")))
        .map(|hz| hz / 1_000_000)
        .or_else(|| {
            read_sysfs_trim(&device_path.join("pp_dpm_sclk"))
                .as_deref()
                .and_then(parse_pp_dpm)
        })
        .unwrap_or(0);

    // Memory clock — freq2_input when exposed, pp_dpm_mclk fallback
    let memory_clock_mhz = hwmon
        .as_ref()
        .and_then(|h| read_sysfs_u64(&h.join("freq2_input")))
        .map(|hz| hz / 1_000_000)
        .or_else(|| {
            read_sysfs_trim(&device_path.join("pp_dpm_mclk"))
                .as_deref()
                .and_then(parse_pp_dpm)
        })
        .unwrap_or(0);

    // Voltages — millivolts (labels on amdgpu hwmon: in0 = vddgfx, in1 = vddnb)
    let vddgfx_mv = hwmon.as_ref().and_then(|h| read_sysfs_u64(&h.join("in0_input")));
    let vddnb_mv = hwmon.as_ref().and_then(|h| read_sysfs_u64(&h.join("in1_input")));

    // Fan — PWM 0-255 → 0-100 %, plus fan1_input/fan1_max in RPM
    let fan_speed_pct = hwmon
        .as_ref()
        .and_then(|h| read_sysfs_u64(&h.join("pwm1")))
        .map(|v| round1(v as f64 / 255.0 * 100.0))
        .unwrap_or(0.0);
    let fan_rpm = hwmon.as_ref().and_then(|h| read_sysfs_u64(&h.join("fan1_input")));
    let fan_max_rpm = hwmon.as_ref().and_then(|h| read_sysfs_u64(&h.join("fan1_max")));

    // PCIe link
    let read_link = |speed_file: &str, width_file: &str| -> (Option<u8>, Option<u32>) {
        let gen = read_sysfs_trim(&device_path.join(speed_file))
            .as_deref()
            .and_then(pcie_gen_from_speed);
        let width = read_sysfs_u64(&device_path.join(width_file)).map(|w| w as u32);
        (gen, width)
    };
    let (pcie_gen, pcie_width) = read_link("current_link_speed", "current_link_width");
    let (pcie_max_gen, pcie_max_width) = read_link("max_link_speed", "max_link_width");

    // PCI identity from uevent (PCI_SLOT_NAME / PCI_ID)
    let pci_slot = read_uevent_field(&device_path, "PCI_SLOT_NAME");
    let pci_device_id = read_uevent_field(&device_path, "PCI_ID")
        .or_else(|| {
            // uevent missing (older kernels / unusual sysfs) — assemble from
            // the vendor/device attribute files
            let dev = read_sysfs_trim(&device_path.join("device"))?;
            Some(format!("1002:{}", dev.trim_start_matches("0x")))
        });

    // amdgpu unique_id (64-bit id, exposed on newer ASICs)
    let unique_id = read_sysfs_trim(&device_path.join("unique_id"));

    // GPU name — product_name → model → hwdata pci.ids lookup → "AMD GPU"
    let name = read_sysfs_trim(&device_path.join("product_name"))
        .or_else(|| read_sysfs_trim(&device_path.join("model")))
        .or_else(|| {
            let dev = read_sysfs_trim(&device_path.join("device"))?;
            let dev = dev.trim_start_matches("0x");
            pci_ids_paths.iter().find_map(|p| lookup_hwdata_name(p, "1002", dev))
        })
        .or_else(|| Some("AMD GPU".to_string()));

    Some(GpuStats {
        name,
        vendor: "AMD".to_string(),
        pci_slot,
        pci_device_id,
        unique_id,
        utilization_gpu,
        utilization_memory,
        memory_used_mb,
        memory_total_mb,
        memory_free_mb,
        gtt_used_mb,
        gtt_total_mb,
        temperature_c,
        temperature_junction_c,
        temperature_memory_c,
        power_w,
        power_cap_w,
        power_cap_max_w,
        core_clock_mhz,
        memory_clock_mhz,
        vddgfx_mv,
        vddnb_mv,
        fan_speed_pct,
        fan_rpm,
        fan_max_rpm,
        pcie_gen,
        pcie_width,
        pcie_max_gen,
        pcie_max_width,
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

    fn read(drm: &Path) -> Option<GpuStats> {
        read_gpu_stats_with(drm, &[])
    }

    #[test]
    fn test_gpu_utilisation() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).expect("should find AMD card");
        assert_eq!(stats.utilization_gpu, 72.0);
    }

    #[test]
    fn test_vram_fields() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).unwrap();
        assert_eq!(stats.memory_total_mb, 8192);
        assert_eq!(stats.memory_used_mb, 2048);
        assert_eq!(stats.memory_free_mb, 6144);
    }

    #[test]
    fn test_memory_utilisation() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).unwrap();
        assert_eq!(stats.utilization_memory, 25.0);
    }

    #[test]
    fn test_temperature() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).unwrap();
        assert_eq!(stats.temperature_c, 65.0);
    }

    #[test]
    fn test_junction_and_memory_temperature() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let hwmon = drm.join("card0/device/hwmon/hwmon1");
        // temp2 = junction 72 500 m°C = 72.5 °C, temp3 = memory 68 000 m°C
        fs::write(hwmon.join("temp2_input"), "72500\n").unwrap();
        fs::write(hwmon.join("temp3_input"), "68000\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.temperature_junction_c, Some(72.5));
        assert_eq!(stats.temperature_memory_c, Some(68.0));
    }

    #[test]
    fn test_power() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).unwrap();
        assert_eq!(stats.power_w, 120.0);
    }

    #[test]
    fn test_power_falls_back_to_power1_input() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let hwmon = drm.join("card0/device/hwmon/hwmon1");
        fs::remove_file(hwmon.join("power1_average")).unwrap();
        // 99 000 000 µW = 99 W
        fs::write(hwmon.join("power1_input"), "99000000\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.power_w, 99.0);
    }

    #[test]
    fn test_power_cap() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let hwmon = drm.join("card0/device/hwmon/hwmon1");
        // 186 W current cap, 200 W max
        fs::write(hwmon.join("power1_cap"), "186000000\n").unwrap();
        fs::write(hwmon.join("power1_cap_max"), "200000000\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.power_cap_w, Some(186.0));
        assert_eq!(stats.power_cap_max_w, Some(200.0));
    }

    #[test]
    fn test_clocks_fall_back_to_pp_dpm() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).unwrap();
        // No freq1_input in the fake sysfs → pp_dpm_* is used
        assert_eq!(stats.core_clock_mhz, 1800);
        assert_eq!(stats.memory_clock_mhz, 1000);
    }

    #[test]
    fn test_clocks_prefer_hwmon_freq() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let hwmon = drm.join("card0/device/hwmon/hwmon1");
        // freq1 = 1 029 000 000 Hz = 1029 MHz, freq2 = 1 000 000 000 Hz
        fs::write(hwmon.join("freq1_input"), "1029000000\n").unwrap();
        fs::write(hwmon.join("freq2_input"), "1000000000\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.core_clock_mhz, 1029);
        assert_eq!(stats.memory_clock_mhz, 1000);
    }

    #[test]
    fn test_fan_speed() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).unwrap();
        // 178 / 255 * 100 = 69.803… → rounds to 69.8
        assert_eq!(stats.fan_speed_pct, 69.8);
    }

    #[test]
    fn test_fan_rpm() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let hwmon = drm.join("card0/device/hwmon/hwmon1");
        fs::write(hwmon.join("fan1_input"), "1650\n").unwrap();
        fs::write(hwmon.join("fan1_max"), "3800\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.fan_rpm, Some(1650));
        assert_eq!(stats.fan_max_rpm, Some(3800));
    }

    #[test]
    fn test_voltages() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let hwmon = drm.join("card0/device/hwmon/hwmon1");
        fs::write(hwmon.join("in0_input"), "1050\n").unwrap();  // vddgfx
        fs::write(hwmon.join("in1_input"), "850\n").unwrap();   // vddnb
        let stats = read(&drm).unwrap();
        assert_eq!(stats.vddgfx_mv, Some(1050));
        assert_eq!(stats.vddnb_mv, Some(850));
    }

    #[test]
    fn test_gtt_memory() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let device = drm.join("card0/device");
        // GTT 128 GiB total, 192 MiB used
        fs::write(device.join("mem_info_gtt_total"), "137438953472\n").unwrap();
        fs::write(device.join("mem_info_gtt_used"), "201326592\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.gtt_total_mb, Some(131072));
        assert_eq!(stats.gtt_used_mb, Some(192));
    }

    #[test]
    fn test_pcie_link() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let device = drm.join("card0/device");
        fs::write(device.join("current_link_speed"), "16.0 GT/s PCIe\n").unwrap();
        fs::write(device.join("current_link_width"), "16\n").unwrap();
        fs::write(device.join("max_link_speed"), "16.0 GT/s PCIe\n").unwrap();
        fs::write(device.join("max_link_width"), "16\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.pcie_gen, Some(4));
        assert_eq!(stats.pcie_width, Some(16));
        assert_eq!(stats.pcie_max_gen, Some(4));
        assert_eq!(stats.pcie_max_width, Some(16));
    }

    #[test]
    fn test_pcie_gen_from_speed() {
        assert_eq!(pcie_gen_from_speed("2.5 GT/s PCIe"), Some(1));
        assert_eq!(pcie_gen_from_speed("8.0 GT/s PCIe"), Some(3));
        assert_eq!(pcie_gen_from_speed("32.0 GT/s PCIe"), Some(5));
        assert_eq!(pcie_gen_from_speed("64.0 GT/s PCIe"), Some(6));
        assert_eq!(pcie_gen_from_speed("garbage"), None);
    }

    #[test]
    fn test_pci_identity() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let device = drm.join("card0/device");
        fs::write(
            device.join("uevent"),
            "DRIVER=amdgpu\nPCI_CLASS=38000\nPCI_ID=1002:1586\nPCI_SLOT_NAME=0000:c5:00.0\n",
        )
        .unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.pci_slot, Some("0000:c5:00.0".to_string()));
        assert_eq!(stats.pci_device_id, Some("1002:1586".to_string()));
    }

    #[test]
    fn test_gpu_name_and_vendor() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let stats = read(&drm).unwrap();
        assert_eq!(stats.name, Some("Radeon RX 7900 XTX".to_string()));
        assert_eq!(stats.vendor, "AMD");
    }

    #[test]
    fn test_name_falls_back_to_hwdata() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let device = drm.join("card0/device");

        // Empty product_name/model (as on Strix Halo) + a matching device id
        fs::write(device.join("product_name"), "\n").unwrap();
        fs::write(device.join("model"), "\n").unwrap();
        fs::write(device.join("device"), "0x1586\n").unwrap();

        // Minimal hwdata-style pci.ids
        let pci_ids = tmp.path().join("pci.ids");
        fs::write(
            &pci_ids,
            "# comment\n1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t1586  Strix Halo [Radeon Graphics]\n\t15bf  Phoenix1\n1000  Broadcom\n\t0001  Other\n",
        )
        .unwrap();

        let stats = read_gpu_stats_with(&drm, &[pci_ids]).unwrap();
        assert_eq!(stats.name, Some("Strix Halo [Radeon Graphics]".to_string()));
        // pci_device_id is assembled from vendor + device attribute files
        assert_eq!(stats.pci_device_id, Some("1002:1586".to_string()));
    }

    #[test]
    fn test_name_defaults_to_amd_gpu() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let device = drm.join("card0/device");
        fs::write(device.join("product_name"), "\n").unwrap();
        fs::write(device.join("model"), "\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.name, Some("AMD GPU".to_string()));
    }

    #[test]
    fn test_unique_id() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        let device = drm.join("card0/device");
        fs::write(device.join("unique_id"), "0123456789abcdef\n").unwrap();
        let stats = read(&drm).unwrap();
        assert_eq!(stats.unique_id, Some("0123456789abcdef".to_string()));
    }

    #[test]
    fn test_non_amd_vendor_returns_none() {
        let tmp = TempDir::new().unwrap();
        let drm = build_fake_sysfs(&tmp);
        // Overwrite vendor with Intel PCI ID
        let vendor_path = drm.join("card0/device/vendor");
        fs::write(&vendor_path, "0x8086\n").unwrap();
        assert!(read(&drm).is_none());
    }
}
