use std::time::Instant;

use metrics::metrics::{battery, cpu, disk, gpu_amd, memory, network, processes, static_info};
use metrics::state::MetricsState;

#[tauri::command]
async fn get_stats(state: tauri::State<'_, MetricsState>) -> Result<serde_json::Value, ()> {
    let cpu    = cpu::read_cpu_stats().await;
    let memory = memory::read_mem_stats();

    // ── Disk ────────────────────────────────────────────────────────────────────
    let now_disk = Instant::now();
    let curr_disk = disk::read_disk_counters();
    let disk_stats = {
        let mut prev = state.prev_disk.lock().await;
        let stats = disk::compute_disk_stats(&prev, &curr_disk, now_disk);
        *prev = disk::DiskSnapshot { counters: curr_disk, at: now_disk };
        stats
    };

    // ── Network ─────────────────────────────────────────────────────────────────
    let now_net = Instant::now();
    let curr_net = network::read_net_counters();
    let net_stats = {
        let mut prev = state.prev_net.lock().await;
        let stats = network::compute_net_stats(&prev, &curr_net, now_net);
        *prev = network::NetSnapshot { counters: curr_net, at: now_net };
        stats
    };

    let process_list = processes::read_processes(&state).await;

    let gpu = gpu_amd::read_gpu_stats();

    let bat = battery::read_battery_stats();

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    Ok(serde_json::json!({
        "cpu":       cpu,
        "memory":    memory,
        "disk":      disk_stats,
        "net":       net_stats,
        "processes": process_list,
        "gpu":       gpu,
        "battery":   bat,
        "ts":        ts,
    }))
}

#[tauri::command]
async fn get_static_info() -> Result<serde_json::Value, ()> {
    let info = static_info::read_static_info();
    Ok(serde_json::to_value(info).unwrap_or(serde_json::Value::Null))
}

#[tauri::command]
async fn get_battery() -> Result<serde_json::Value, ()> {
    let bat = battery::read_battery_stats();
    Ok(serde_json::to_value(bat).unwrap_or(serde_json::Value::Null))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(MetricsState::default())
        .invoke_handler(tauri::generate_handler![get_stats, get_static_info, get_battery])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
