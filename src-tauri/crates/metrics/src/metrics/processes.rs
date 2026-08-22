use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

use libc::{_SC_CLK_TCK, _SC_PAGESIZE};
use serde::Serialize;

use crate::state::{MetricsState, ProcessSnapshot};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cmd: String,
    pub cpu: f32,
    pub mem_bytes: u64,
    pub status: String,
    pub user: String,
}

#[derive(Debug, Clone, PartialEq)]
struct StatInfo {
    pid: u32,
    name: String,
    state: String,
    total_ticks: u64,
    rss_pages: u64,
}

fn page_size() -> u64 {
    let size = unsafe { libc::sysconf(_SC_PAGESIZE) };
    if size > 0 {
        size as u64
    } else {
        4096
    }
}

fn clock_ticks_per_second() -> f64 {
    let ticks = unsafe { libc::sysconf(_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as f64
    } else {
        100.0
    }
}

fn parse_stat_line(line: &str) -> Option<StatInfo> {
    let open = line.find(" (")?;
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }

    let pid = line[..open].trim().parse().ok()?;
    let name = line[open + 2..close].to_string();
    let rest = line.get(close + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    if fields.len() <= 21 {
        return None;
    }

    let state = fields[0].to_string();
    let utime: u64 = fields[11].parse().ok()?;
    let stime: u64 = fields[12].parse().ok()?;
    let rss_pages: u64 = fields[21].parse().ok()?;

    Some(StatInfo {
        pid,
        name,
        state,
        total_ticks: utime + stime,
        rss_pages,
    })
}

fn parse_cmdline_bytes(bytes: &[u8]) -> String {
    let mut cmd = String::from_utf8_lossy(bytes).replace('\0', " ");
    while cmd.ends_with(' ') {
        cmd.pop();
    }
    cmd
}

fn read_cmdline(proc_dir: &Path, fallback_name: &str) -> String {
    match fs::read(proc_dir.join("cmdline")) {
        Ok(bytes) => {
            let cmd = parse_cmdline_bytes(&bytes);
            if cmd.is_empty() {
                fallback_name.to_string()
            } else {
                cmd
            }
        }
        Err(_) => fallback_name.to_string(),
    }
}

fn read_uid(proc_dir: &Path) -> Option<u32> {
    let status = fs::read_to_string(proc_dir.join("status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn load_passwd_cache() -> HashMap<u32, String> {
    let mut cache = HashMap::new();
    let Ok(passwd) = fs::read_to_string("/etc/passwd") else {
        return cache;
    };

    for line in passwd.lines() {
        let mut parts = line.split(':');
        let Some(username) = parts.next() else { continue };
        let _password = parts.next();
        let Some(uid) = parts.next().and_then(|uid| uid.parse::<u32>().ok()) else {
            continue;
        };
        cache.insert(uid, username.to_string());
    }

    cache
}

fn lookup_user(uid: Option<u32>, users: &HashMap<u32, String>) -> String {
    uid.and_then(|id| users.get(&id).cloned())
        .unwrap_or_else(|| "—".to_string())
}

fn round_one_decimal(value: f32) -> f32 {
    (value * 10.0).round() / 10.0
}

fn compute_cpu_percent(prev_ticks: Option<u64>, curr_ticks: u64, elapsed: f64, clk_tck: f64) -> f32 {
    if elapsed <= 0.0 {
        return 0.0;
    }
    let Some(prev_ticks) = prev_ticks else {
        return 0.0;
    };
    let elapsed_ticks = elapsed * clk_tck;
    if elapsed_ticks <= 0.0 {
        return 0.0;
    }
    let delta_ticks = curr_ticks.saturating_sub(prev_ticks) as f64;
    ((delta_ticks / elapsed_ticks) * 100.0) as f32
}

fn build_process_list(
    proc_root: &Path,
    prev: &ProcessSnapshot,
    now: Instant,
    page_size: u64,
    clk_tck: f64,
) -> io::Result<(Vec<ProcessInfo>, HashMap<u32, u64>)> {
    let mut processes = Vec::new();
    let mut current_ticks = HashMap::new();
    let elapsed = now.duration_since(prev.at).as_secs_f64();

    for entry in fs::read_dir(proc_root)? {
        let Ok(entry) = entry else { continue };
        let file_name = entry.file_name();
        let Some(pid_str) = file_name.to_str() else { continue };
        if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }

        let proc_dir = entry.path();
        let Ok(stat_line) = fs::read_to_string(proc_dir.join("stat")) else {
            continue;
        };
        let Some(stat) = parse_stat_line(stat_line.trim_end()) else {
            continue;
        };

        let cpu = round_one_decimal(compute_cpu_percent(
            prev.ticks.get(&stat.pid).copied(),
            stat.total_ticks,
            elapsed,
            clk_tck,
        ));
        current_ticks.insert(stat.pid, stat.total_ticks);

        processes.push(ProcessInfo {
            pid: stat.pid,
            name: stat.name.clone(),
            cmd: read_cmdline(&proc_dir, &stat.name),
            cpu,
            mem_bytes: stat.rss_pages.saturating_mul(page_size),
            status: stat.state,
            user: lookup_user(read_uid(&proc_dir), &prev.users),
        });
    }

    processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    processes.truncate(40);

    Ok((processes, current_ticks))
}

pub async fn read_processes(state: &MetricsState) -> Vec<ProcessInfo> {
    let now = Instant::now();
    let page_size = page_size();
    let clk_tck = clock_ticks_per_second();

    let mut prev = state.prev_procs.lock().await;
    if prev.users.is_empty() {
        prev.users = load_passwd_cache();
    }

    let result = build_process_list(Path::new("/proc"), &prev, now, page_size, clk_tck);
    match result {
        Ok((processes, current_ticks)) => {
            prev.ticks = current_ticks;
            prev.at = now;
            processes
        }
        Err(_) => {
            prev.ticks.clear();
            prev.at = now;
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn make_stat_line(utime: u64, stime: u64) -> String {
        let mut fields = vec!["0".to_string(); 22];
        fields[0] = "S".to_string();
        fields[11] = utime.to_string();
        fields[12] = stime.to_string();
        fields[21] = "42".to_string();
        format!("1234 (my (process) name) {}", fields.join(" "))
    }

    #[test]
    fn test_parse_stat_line_with_spaces_and_parentheses() {
        let stat = parse_stat_line(&make_stat_line(111, 222)).expect("parsed stat line");
        assert_eq!(stat.pid, 1234);
        assert_eq!(stat.name, "my (process) name");
        assert_eq!(stat.state, "S");
        assert_eq!(stat.total_ticks, 333);
        assert_eq!(stat.rss_pages, 42);
    }

    #[test]
    fn test_cpu_delta_arithmetic() {
        let cpu = compute_cpu_percent(Some(100), 115, 0.3, 100.0);
        assert!((cpu - 50.0).abs() < 0.001, "cpu={cpu}");
    }

    #[test]
    fn test_cmdline_nul_replacement() {
        let cmd = parse_cmdline_bytes(b"bash\0-c\0echo\0");
        assert_eq!(cmd, "bash -c echo");
    }

    #[test]
    fn test_first_sample_has_zero_cpu() {
        let cpu = compute_cpu_percent(None, 115, 0.1, 100.0);
        assert_eq!(cpu, 0.0);
    }

    #[test]
    fn test_build_process_list_uses_previous_ticks() {
        let temp = std::env::temp_dir().join(format!(
            "metrics-processes-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(temp.join("1234")).unwrap();
        fs::write(temp.join("1234/stat"), make_stat_line(110, 5)).unwrap();
        fs::write(temp.join("1234/cmdline"), b"bash\0-c\0echo\0").unwrap();
        fs::write(temp.join("1234/status"), "Name:\tbash\nUid:\t1000\t1000\t1000\t1000\n").unwrap();

        let mut prev = ProcessSnapshot::default();
        prev.at = Instant::now();
        prev.at -= Duration::from_millis(100);
        prev.ticks.insert(1234, 100);
        prev.users.insert(1000, "markus".to_string());

        let (processes, ticks) = build_process_list(&temp, &prev, Instant::now(), 4096, 100.0).unwrap();
        assert_eq!(ticks.get(&1234), Some(&115));
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].cmd, "bash -c echo");
        assert_eq!(processes[0].user, "markus");
        assert_eq!(processes[0].status, "S");
        assert_eq!(processes[0].mem_bytes, 42 * 4096);

        let _ = fs::remove_dir_all(temp);
    }
}
