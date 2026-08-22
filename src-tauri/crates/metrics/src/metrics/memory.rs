use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::Path;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemStats {
    pub total_bytes:  u64,
    pub used_bytes:   u64,
    pub active_bytes: u64,
    pub free_bytes:   u64,
    pub avail_bytes:  u64,
    pub swap_total:   u64,
    pub swap_used:    u64,
    pub used_percent: f32,
}

fn parse_meminfo<R: io::Read>(reader: R) -> HashMap<String, u64> {
    let buf = io::BufReader::new(reader);
    let mut map = HashMap::new();
    for line in buf.lines().flatten() {
        // Format: "MemTotal:       16384 kB"
        let mut parts = line.splitn(2, ':');
        let key = match parts.next() {
            Some(k) => k.trim().to_string(),
            None => continue,
        };
        let val_str = match parts.next() {
            Some(v) => v.trim(),
            None => continue,
        };
        // Value is in kB; strip the unit and convert to bytes
        let numeric: u64 = val_str
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        // All meminfo values are in kibibytes
        map.insert(key, numeric * 1024);
    }
    map
}

fn build_stats(map: &HashMap<String, u64>) -> MemStats {
    let total  = *map.get("MemTotal").unwrap_or(&0);
    let free   = *map.get("MemFree").unwrap_or(&0);
    let avail  = *map.get("MemAvailable").unwrap_or(&0);
    let active = *map.get("Active").unwrap_or(&0);
    let swap_total = *map.get("SwapTotal").unwrap_or(&0);
    let swap_free  = *map.get("SwapFree").unwrap_or(&0);

    let used       = total.saturating_sub(free);
    let swap_used  = swap_total.saturating_sub(swap_free);
    let used_pct   = if total > 0 {
        ((used as f64 / total as f64) * 1000.0).round() / 10.0
    } else {
        0.0
    };

    MemStats {
        total_bytes:  total,
        used_bytes:   used,
        active_bytes: active,
        free_bytes:   free,
        avail_bytes:  avail,
        swap_total,
        swap_used,
        used_percent: used_pct as f32,
    }
}

/// Read memory stats from `/proc/meminfo`.
pub fn read_mem_stats() -> MemStats {
    read_mem_stats_from_path(Path::new("/proc/meminfo"))
}

/// Same as `read_mem_stats` but reads from an arbitrary path (used in tests).
pub fn read_mem_stats_from_path(path: &Path) -> MemStats {
    match std::fs::File::open(path) {
        Ok(f) => {
            let map = parse_meminfo(f);
            build_stats(&map)
        }
        Err(_) => build_stats(&HashMap::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_MEMINFO: &str = "\
MemTotal:       16384000 kB\n\
MemFree:         2048000 kB\n\
MemAvailable:    4096000 kB\n\
Active:          6144000 kB\n\
Inactive:        2000000 kB\n\
SwapTotal:        524288 kB\n\
SwapFree:         262144 kB\n";

    #[test]
    fn test_parse_meminfo_keys() {
        let map = parse_meminfo(FAKE_MEMINFO.as_bytes());
        assert_eq!(map["MemTotal"],     16384000 * 1024);
        assert_eq!(map["MemFree"],       2048000 * 1024);
        assert_eq!(map["MemAvailable"],  4096000 * 1024);
        assert_eq!(map["Active"],        6144000 * 1024);
        assert_eq!(map["SwapTotal"],      524288 * 1024);
        assert_eq!(map["SwapFree"],       262144 * 1024);
    }

    #[test]
    fn test_build_stats_values() {
        let map = parse_meminfo(FAKE_MEMINFO.as_bytes());
        let stats = build_stats(&map);

        let total_kb: u64 = 16384000;
        let free_kb:  u64 =  2048000;
        let used_kb         = total_kb - free_kb; // 14336000

        assert_eq!(stats.total_bytes,  total_kb * 1024);
        assert_eq!(stats.free_bytes,   free_kb  * 1024);
        assert_eq!(stats.used_bytes,   used_kb  * 1024);
        assert_eq!(stats.avail_bytes,  4096000  * 1024);
        assert_eq!(stats.active_bytes, 6144000  * 1024);
        assert_eq!(stats.swap_total,    524288  * 1024);
        assert_eq!(stats.swap_used,     262144  * 1024); // 524288 - 262144

        // usedPercent = round(14336000/16384000 * 1000) / 10  = round(875.0) / 10 = 87.5
        assert_eq!(stats.used_percent, 87.5_f32);
    }

    #[test]
    fn test_zero_total_guard() {
        let map = HashMap::new();
        let stats = build_stats(&map);
        assert_eq!(stats.used_percent, 0.0);
        assert_eq!(stats.total_bytes,  0);
    }
}
