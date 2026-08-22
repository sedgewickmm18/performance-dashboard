use std::fs;
use std::io::{self, BufRead};
use std::path::Path;
use std::time::Duration;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuStats {
    pub load_percent: f32,
    pub user_percent: f32,
    pub sys_percent: f32,
    pub cores: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
struct CpuTimes {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuTimes {
    fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    fn idle_total(&self) -> u64 {
        self.idle + self.iowait
    }

    fn user_total(&self) -> u64 {
        self.user + self.nice
    }

    fn sys_total(&self) -> u64 {
        self.system + self.irq + self.softirq
    }
}

fn round1(v: f64) -> f32 {
    ((v * 10.0).round() / 10.0) as f32
}

fn parse_stat<R: io::Read>(reader: R) -> Vec<CpuTimes> {
    let buf = io::BufReader::new(reader);
    let mut result = Vec::new();
    for line in buf.lines().flatten() {
        if !line.starts_with("cpu") {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }
        let t = CpuTimes {
            user:    parts[1].parse().unwrap_or(0),
            nice:    parts[2].parse().unwrap_or(0),
            system:  parts[3].parse().unwrap_or(0),
            idle:    parts[4].parse().unwrap_or(0),
            iowait:  parts[5].parse().unwrap_or(0),
            irq:     parts[6].parse().unwrap_or(0),
            softirq: parts[7].parse().unwrap_or(0),
            steal:   if parts.len() > 8 { parts[8].parse().unwrap_or(0) } else { 0 },
        };
        result.push(t);
    }
    result
}

fn read_stat_from_path(path: &Path) -> Vec<CpuTimes> {
    match fs::File::open(path) {
        Ok(f) => parse_stat(f),
        Err(_) => vec![],
    }
}

fn compute_stats(before: &[CpuTimes], after: &[CpuTimes]) -> CpuStats {
    // Index 0 = aggregate "cpu" line, indices 1..N = per-core lines
    let (agg_load, agg_user, agg_sys) = if before.len() >= 1 && after.len() >= 1 {
        delta_percents(&before[0], &after[0])
    } else {
        (0.0, 0.0, 0.0)
    };

    let cores: Vec<f32> = before
        .iter()
        .zip(after.iter())
        .skip(1) // skip aggregate line
        .map(|(b, a)| {
            let (load, _, _) = delta_percents(b, a);
            round1(load)
        })
        .collect();

    CpuStats {
        load_percent: round1(agg_load),
        user_percent: round1(agg_user),
        sys_percent: round1(agg_sys),
        cores,
    }
}

fn delta_percents(before: &CpuTimes, after: &CpuTimes) -> (f64, f64, f64) {
    let total_delta = after.total().saturating_sub(before.total()) as f64;
    if total_delta == 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let idle_delta = after.idle_total().saturating_sub(before.idle_total()) as f64;
    let user_delta = after.user_total().saturating_sub(before.user_total()) as f64;
    let sys_delta  = after.sys_total().saturating_sub(before.sys_total()) as f64;

    let load = (1.0 - idle_delta / total_delta) * 100.0;
    let user = (user_delta / total_delta) * 100.0;
    let sys  = (sys_delta  / total_delta) * 100.0;
    (load, user, sys)
}

/// Read CPU stats from `/proc/stat` with a 100 ms delta sample.
pub async fn read_cpu_stats() -> CpuStats {
    read_cpu_stats_from_path(Path::new("/proc/stat")).await
}

/// Same as `read_cpu_stats` but reads from an arbitrary path (used in tests).
pub async fn read_cpu_stats_from_path(path: &Path) -> CpuStats {
    let before = read_stat_from_path(path);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after = read_stat_from_path(path);
    compute_stats(&before, &after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_times(user: u64, nice: u64, system: u64, idle: u64) -> CpuTimes {
        CpuTimes { user, nice, system, idle, ..Default::default() }
    }

    #[test]
    fn test_delta_50pct_load() {
        // 100 ticks total: 50 idle, 50 active
        let before = make_times(0, 0, 0, 100);
        let after  = make_times(50, 0, 0, 150); // +50 user, +50 idle → 200 total
        let (load, user, sys) = delta_percents(&before, &after);
        // total_delta=100, idle_delta=50  → load=50%
        assert_eq!((load * 10.0).round() / 10.0, 50.0);
        assert_eq!((user * 10.0).round() / 10.0, 50.0);
        assert_eq!((sys  * 10.0).round() / 10.0,  0.0);
    }

    #[test]
    fn test_parse_stat_aggregate_and_cores() {
        let content = "\
cpu  100 0 50 200 0 0 0 0\n\
cpu0  50 0 25 100 0 0 0 0\n\
cpu1  50 0 25 100 0 0 0 0\n\
intr 12345\n";
        let times = parse_stat(content.as_bytes());
        assert_eq!(times.len(), 3); // aggregate + 2 cores
        assert_eq!(times[0].user, 100);
        assert_eq!(times[1].idle, 100);
        assert_eq!(times[2].system, 25);
    }

    #[test]
    fn test_compute_stats_two_cores() {
        let before = vec![
            make_times(0, 0, 0, 1000),   // aggregate
            make_times(0, 0, 0, 500),    // core0
            make_times(0, 0, 0, 500),    // core1
        ];
        let after = vec![
            make_times(100, 0, 0, 1000), // aggregate: +100 user, +0 idle → 50% load
            make_times(100, 0, 0,  500), // core0: 100/(600) ~16.7%
            make_times(  0, 0, 0,  600), // core1: 100 idle delta out of 100 total → 0%
        ];
        let stats = compute_stats(&before, &after);
        // aggregate: total_delta=200, idle_delta=0, load=100%... wait let's be precise:
        // after[0].total() = 100+0+0+1000 = 1100, before[0].total() = 0+0+0+1000 = 1000
        // total_delta=100, idle_delta=0 → load=100%
        assert_eq!(stats.load_percent, 100.0);
        assert_eq!(stats.user_percent, 100.0);
        assert_eq!(stats.cores.len(), 2);
        // core0: total_delta=100, idle_delta=0 → 100%
        assert_eq!(stats.cores[0], 100.0);
        // core1: total_delta=100, idle_delta=100 → 0%
        assert_eq!(stats.cores[1], 0.0);
    }

    #[test]
    fn test_zero_delta_guard() {
        let before = make_times(100, 0, 50, 200);
        let after  = make_times(100, 0, 50, 200); // identical → zero delta
        let (load, user, sys) = delta_percents(&before, &after);
        assert_eq!(load, 0.0);
        assert_eq!(user, 0.0);
        assert_eq!(sys,  0.0);
    }
}
