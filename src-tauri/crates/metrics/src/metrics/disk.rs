use std::io::{self, BufRead};
use std::time::Instant;
use serde::Serialize;

/// Cumulative counters read from `/proc/diskstats`.
#[derive(Debug, Clone, Default)]
pub struct DiskCounters {
    pub reads_completed:  u64,
    pub sectors_read:     u64,
    pub writes_completed: u64,
    pub sectors_written:  u64,
}

/// Previous snapshot held in `MetricsState`.
#[derive(Debug, Clone)]
pub struct DiskSnapshot {
    pub counters: DiskCounters,
    pub at:       Instant,
}

impl Default for DiskSnapshot {
    fn default() -> Self {
        DiskSnapshot { counters: DiskCounters::default(), at: Instant::now() }
    }
}

/// Return value from `compute_disk_stats`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskStats {
    pub read_bps:   f64,
    pub write_bps:  f64,
    pub read_ops:   f64,
    pub write_ops:  f64,
}

/// Returns `true` for device names that should be excluded (not physical block devices).
fn skip_device(name: &str) -> bool {
    name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("dm-")
        || name.starts_with("sr")
        || name.starts_with("fd")
}

/// Parse `/proc/diskstats` content and sum counters across physical devices.
pub fn parse_diskstats<R: io::Read>(reader: R) -> DiskCounters {
    let buf = io::BufReader::new(reader);
    let mut total = DiskCounters::default();
    for line in buf.lines().flatten() {
        // columns (1-based): 1=major, 2=minor, 3=name, 4=reads_completed, ...
        // 6=sectors_read, 8=writes_completed, 10=sectors_written
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }
        let name = parts[2];
        if skip_device(name) {
            continue;
        }
        // Only include whole-disk devices (no partition suffix like sda1, nvme0n1p1)
        // A partition has a non-zero minor where the parent disk has minor 0,
        // but the simplest heuristic is: if the name ends with a digit AND the
        // parent without the trailing digit(s) is also a physical name, it's a
        // partition. However the spec says "sum across physical block devices only"
        // so we skip names whose last character is a digit and are not nvme whole disks.
        // nvme whole disks look like nvme0n1 (ends in digit but IS a whole disk).
        // We key on the minor number being 0 which is the standard for whole disks,
        // OR: simpler — just check that the minor (parts[1]) is "0" for sdX/hdX,
        // and accept nvme patterns (nvme\d+n\d+) explicitly.
        //
        // Simplest correct approach: only include entries whose minor == 0 for
        // traditional devices. But nvme whole-disk minors are also 0. Partition
        // minors for sdX are >= 1, for nvme are 1,2,...
        let minor: u64 = parts[1].parse().unwrap_or(1);
        if minor != 0 {
            continue;
        }
        total.reads_completed  += parts[3].parse::<u64>().unwrap_or(0);
        total.sectors_read     += parts[5].parse::<u64>().unwrap_or(0);
        total.writes_completed += parts[7].parse::<u64>().unwrap_or(0);
        total.sectors_written  += parts[9].parse::<u64>().unwrap_or(0);
    }
    total
}

/// Compute rate stats given a previous and current snapshot.
/// Returns all-zeros if `elapsed` is zero or no previous snapshot is available.
pub fn compute_disk_stats(prev: &DiskSnapshot, curr_counters: &DiskCounters, now: Instant) -> DiskStats {
    let secs = now.duration_since(prev.at).as_secs_f64();
    if secs <= 0.0 {
        return DiskStats { read_bps: 0.0, write_bps: 0.0, read_ops: 0.0, write_ops: 0.0 };
    }
    const SECTOR_SIZE: f64 = 512.0;
    let read_sectors = curr_counters.sectors_read.saturating_sub(prev.counters.sectors_read) as f64;
    let write_sectors = curr_counters.sectors_written.saturating_sub(prev.counters.sectors_written) as f64;
    let read_ops = curr_counters.reads_completed.saturating_sub(prev.counters.reads_completed) as f64;
    let write_ops = curr_counters.writes_completed.saturating_sub(prev.counters.writes_completed) as f64;

    DiskStats {
        read_bps:  read_sectors  * SECTOR_SIZE / secs,
        write_bps: write_sectors * SECTOR_SIZE / secs,
        read_ops:  read_ops  / secs,
        write_ops: write_ops / secs,
    }
}

/// Read current disk counters from `/proc/diskstats`.
pub fn read_disk_counters() -> DiskCounters {
    match std::fs::File::open("/proc/diskstats") {
        Ok(f) => parse_diskstats(f),
        Err(_) => DiskCounters::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const DISKSTATS_A: &str = "\
   8   0 sda 1000 0 8000 0 500 0 4000 0 0 0 0\n\
   8   1 sda1 200 0 1600 0 100 0 800 0 0 0 0\n\
   7   0 loop0 0 0 0 0 0 0 0 0 0 0 0\n\
 253   0 dm-0 800 0 6400 0 400 0 3200 0 0 0 0\n\
 259   0 nvme0n1 2000 0 16000 0 1000 0 8000 0 0 0 0\n\
 259   1 nvme0n1p1 100 0 800 0 50 0 400 0 0 0 0\n";

    const DISKSTATS_B: &str = "\
   8   0 sda 1100 0 9000 0 600 0 5000 0 0 0 0\n\
   8   1 sda1 220 0 1760 0 110 0 880 0 0 0 0\n\
   7   0 loop0 0 0 0 0 0 0 0 0 0 0 0\n\
 253   0 dm-0 900 0 7200 0 500 0 4000 0 0 0 0\n\
 259   0 nvme0n1 2200 0 17600 0 1100 0 9000 0 0 0 0\n\
 259   1 nvme0n1p1 110 0 880 0 55 0 440 0 0 0 0\n";

    #[test]
    fn test_parse_diskstats_skips_partitions_and_excluded() {
        let c = parse_diskstats(DISKSTATS_A.as_bytes());
        // sda (minor 0): reads=1000, sectors_read=8000, writes=500, sectors_written=4000
        // loop0: skipped (name prefix)
        // dm-0: skipped (name prefix)
        // nvme0n1 (minor 0): reads=2000, sectors_read=16000, writes=1000, sectors_written=8000
        // sda1, nvme0n1p1: minor != 0, skipped
        assert_eq!(c.reads_completed,  1000 + 2000);
        assert_eq!(c.sectors_read,     8000 + 16000);
        assert_eq!(c.writes_completed, 500  + 1000);
        assert_eq!(c.sectors_written,  4000 + 8000);
    }

    #[test]
    fn test_compute_disk_stats_delta() {
        let counters_a = parse_diskstats(DISKSTATS_A.as_bytes());
        let counters_b = parse_diskstats(DISKSTATS_B.as_bytes());

        let t0 = Instant::now();
        let prev = DiskSnapshot { counters: counters_a, at: t0 };
        // Simulate 1 second elapsed
        let t1 = t0 + Duration::from_secs(1);
        let stats = compute_disk_stats(&prev, &counters_b, t1);

        // sda delta: sectors_read = 9000-8000=1000, sectors_written = 5000-4000=1000
        // nvme delta: sectors_read = 17600-16000=1600, sectors_written = 9000-8000=1000
        // total sectors_read_delta = 2600, sectors_written_delta = 2000
        // read_bps = 2600 * 512 / 1 = 1_331_200
        // write_bps = 2000 * 512 / 1 = 1_024_000
        assert!((stats.read_bps  - 2600.0 * 512.0).abs() < 1.0, "read_bps={}", stats.read_bps);
        assert!((stats.write_bps - 2000.0 * 512.0).abs() < 1.0, "write_bps={}", stats.write_bps);

        // read_ops_delta: sda(1100-1000=100) + nvme(2200-2000=200) = 300 over 1s → 300 ops/s
        // write_ops_delta: sda(600-500=100) + nvme(1100-1000=100) = 200 over 1s → 200 ops/s
        assert!((stats.read_ops  - 300.0).abs() < 0.01, "read_ops={}", stats.read_ops);
        assert!((stats.write_ops - 200.0).abs() < 0.01, "write_ops={}", stats.write_ops);
    }

    #[test]
    fn test_first_call_zero_elapsed_returns_zeros() {
        let counters = parse_diskstats(DISKSTATS_A.as_bytes());
        let t = Instant::now();
        let prev = DiskSnapshot { counters: DiskCounters::default(), at: t };
        let stats = compute_disk_stats(&prev, &counters, t);
        assert_eq!(stats.read_bps, 0.0);
        assert_eq!(stats.write_bps, 0.0);
    }
}
