use std::fs;
use std::path::{Path, PathBuf};
use serde::Serialize;

// ── Data types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryStats {
    pub has_battery: bool,
    pub percent: u32,
    pub is_charging: bool,
}

// ── Detection ───────────────────────────────────────────────────────────────

/// Find a battery directory under `power_supply_root` (normally
/// `/sys/class/power_supply`).  Tries `BAT0`, then `BAT1`, then any
/// directory whose name starts with `BAT`.
fn find_battery(power_supply_root: &Path) -> Option<PathBuf> {
    // Preferred names first
    for name in &["BAT0", "BAT1"] {
        let p = power_supply_root.join(name);
        if p.is_dir() {
            return Some(p);
        }
    }
    // Fallback: any BAT* entry
    if let Ok(entries) = fs::read_dir(power_supply_root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("BAT") && entry.path().is_dir() {
                return Some(entry.path());
            }
        }
    }
    None
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Read battery stats from the real sysfs root `/sys/class/power_supply`.
pub fn read_battery_stats() -> BatteryStats {
    read_battery_stats_from(Path::new("/sys/class/power_supply"))
}

/// Same as [`read_battery_stats`] but accepts an arbitrary root for testing.
pub fn read_battery_stats_from(power_supply_root: &Path) -> BatteryStats {
    let bat_path = match find_battery(power_supply_root) {
        Some(p) => p,
        None => return BatteryStats { has_battery: false, percent: 0, is_charging: false },
    };

    let percent = fs::read_to_string(bat_path.join("capacity"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let status = fs::read_to_string(bat_path.join("status"))
        .unwrap_or_default();
    let status = status.trim();
    let is_charging = status == "Charging" || status == "Full";

    BatteryStats { has_battery: true, percent, is_charging }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_bat(root: &Path, name: &str, capacity: &str, status: &str) {
        let bat = root.join(name);
        fs::create_dir_all(&bat).unwrap();
        fs::write(bat.join("capacity"), capacity).unwrap();
        fs::write(bat.join("status"), status).unwrap();
    }

    #[test]
    fn test_discharging() {
        let tmp = TempDir::new().unwrap();
        make_bat(tmp.path(), "BAT0", "85\n", "Discharging\n");
        let s = read_battery_stats_from(tmp.path());
        assert!(s.has_battery);
        assert_eq!(s.percent, 85);
        assert!(!s.is_charging);
    }

    #[test]
    fn test_charging() {
        let tmp = TempDir::new().unwrap();
        make_bat(tmp.path(), "BAT0", "60\n", "Charging\n");
        let s = read_battery_stats_from(tmp.path());
        assert!(s.has_battery);
        assert!(s.is_charging);
    }

    #[test]
    fn test_no_battery() {
        let tmp = TempDir::new().unwrap();
        let s = read_battery_stats_from(tmp.path());
        assert!(!s.has_battery);
        assert_eq!(s.percent, 0);
        assert!(!s.is_charging);
    }
}
