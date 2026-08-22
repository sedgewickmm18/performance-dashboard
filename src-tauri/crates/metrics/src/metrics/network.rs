use std::io::{self, BufRead};
use std::time::Instant;
use serde::Serialize;

/// Cumulative per-interface counters read from `/proc/net/dev`.
#[derive(Debug, Clone, Default)]
pub struct NetCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Names of included (non-loopback) interfaces.
    pub ifaces: Vec<String>,
}

/// Previous snapshot held in `MetricsState`.
#[derive(Debug, Clone)]
pub struct NetSnapshot {
    pub counters: NetCounters,
    pub at:       Instant,
}

impl Default for NetSnapshot {
    fn default() -> Self {
        NetSnapshot { counters: NetCounters::default(), at: Instant::now() }
    }
}

/// Return value from `compute_net_stats`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetStats {
    pub rx_bps:   f64,
    pub tx_bps:   f64,
    pub rx_total: u64,
    pub tx_total: u64,
    pub ifaces:   Vec<String>,
}

/// Parse `/proc/net/dev` content, skipping the two header lines and the `lo` interface.
///
/// `/proc/net/dev` format (columns after the interface name):
/// rx_bytes rx_packets rx_errs rx_drop rx_fifo rx_frame rx_compressed rx_multicast
/// tx_bytes tx_packets tx_errs tx_drop tx_fifo tx_colls tx_carrier tx_compressed
pub fn parse_net_dev<R: io::Read>(reader: R) -> NetCounters {
    let buf = io::BufReader::new(reader);
    let mut total = NetCounters::default();
    for (i, line) in buf.lines().flatten().enumerate() {
        // Skip the two header lines
        if i < 2 {
            continue;
        }
        // Format: "  eth0:  12345  ..."
        let line = line.trim();
        let colon = match line.find(':') {
            Some(p) => p,
            None => continue,
        };
        let iface = line[..colon].trim();
        if iface == "lo" {
            continue;
        }
        let rest = line[colon + 1..].trim();
        let parts: Vec<&str> = rest.split_whitespace().collect();
        // rx_bytes is the first field, tx_bytes is the 9th field (index 8)
        if parts.len() < 9 {
            continue;
        }
        let rx: u64 = parts[0].parse().unwrap_or(0);
        let tx: u64 = parts[8].parse().unwrap_or(0);
        total.rx_bytes += rx;
        total.tx_bytes += tx;
        total.ifaces.push(iface.to_string());
    }
    total
}

/// Compute rate stats given a previous and current snapshot.
pub fn compute_net_stats(prev: &NetSnapshot, curr: &NetCounters, now: Instant) -> NetStats {
    let secs = now.duration_since(prev.at).as_secs_f64();
    let (rx_bps, tx_bps) = if secs > 0.0 {
        let rx_delta = curr.rx_bytes.saturating_sub(prev.counters.rx_bytes) as f64;
        let tx_delta = curr.tx_bytes.saturating_sub(prev.counters.tx_bytes) as f64;
        (rx_delta / secs, tx_delta / secs)
    } else {
        (0.0, 0.0)
    };

    NetStats {
        rx_bps,
        tx_bps,
        rx_total: curr.rx_bytes,
        tx_total: curr.tx_bytes,
        ifaces:   curr.ifaces.clone(),
    }
}

/// Read current network counters from `/proc/net/dev`.
pub fn read_net_counters() -> NetCounters {
    match std::fs::File::open("/proc/net/dev") {
        Ok(f) => parse_net_dev(f),
        Err(_) => NetCounters::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // Fake /proc/net/dev with two real interfaces and lo (which must be excluded)
    const NET_DEV_A: &str = "\
Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo:  100000    500    0    0    0     0          0         0   100000     500    0    0    0     0       0          0\n\
  eth0: 1000000   5000    0    0    0     0          0         0   500000    2500    0    0    0     0       0          0\n\
  wlan0:  200000   1000    0    0    0     0          0         0   100000     500    0    0    0     0       0          0\n";

    const NET_DEV_B: &str = "\
Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo:  110000    510    0    0    0     0          0         0   110000     510    0    0    0     0       0          0\n\
  eth0: 1100000   5100    0    0    0     0          0         0   600000    2600    0    0    0     0       0          0\n\
  wlan0:  250000   1050    0    0    0     0          0         0   120000     520    0    0    0     0       0          0\n";

    #[test]
    fn test_parse_net_dev_excludes_lo() {
        let c = parse_net_dev(NET_DEV_A.as_bytes());
        // lo must be excluded; eth0 + wlan0 included
        assert_eq!(c.rx_bytes, 1000000 + 200000);
        assert_eq!(c.tx_bytes,  500000 + 100000);
        assert_eq!(c.ifaces.len(), 2);
        assert!(c.ifaces.contains(&"eth0".to_string()));
        assert!(c.ifaces.contains(&"wlan0".to_string()));
        assert!(!c.ifaces.contains(&"lo".to_string()));
    }

    #[test]
    fn test_compute_net_stats_delta() {
        let counters_a = parse_net_dev(NET_DEV_A.as_bytes());
        let counters_b = parse_net_dev(NET_DEV_B.as_bytes());

        let t0 = Instant::now();
        let prev = NetSnapshot { counters: counters_a, at: t0 };
        let t1 = t0 + Duration::from_secs(1);
        let stats = compute_net_stats(&prev, &counters_b, t1);

        // rx_delta = (1100000+250000) - (1000000+200000) = 150000 over 1s
        // tx_delta = (600000+120000) - (500000+100000) = 120000 over 1s
        assert!((stats.rx_bps - 150000.0).abs() < 0.01, "rx_bps={}", stats.rx_bps);
        assert!((stats.tx_bps - 120000.0).abs() < 0.01, "tx_bps={}", stats.tx_bps);

        // totals are current raw counters
        assert_eq!(stats.rx_total, 1100000 + 250000);
        assert_eq!(stats.tx_total,  600000 + 120000);

        // ifaces from current snapshot
        assert_eq!(stats.ifaces.len(), 2);
    }

    #[test]
    fn test_first_call_zero_elapsed_returns_zeros() {
        let counters = parse_net_dev(NET_DEV_A.as_bytes());
        let t = Instant::now();
        let prev = NetSnapshot { counters: NetCounters::default(), at: t };
        let stats = compute_net_stats(&prev, &counters, t);
        assert_eq!(stats.rx_bps, 0.0);
        assert_eq!(stats.tx_bps, 0.0);
    }
}
