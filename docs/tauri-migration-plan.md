# Tauri Migration Plan

## Overview

Convert the existing Node.js/Express system-monitoring dashboard into a self-contained
Tauri desktop application for Linux. The Node.js backend (Express, systeminformation,
node-gpu) is eliminated entirely and replaced with native Rust Tauri commands that read
metrics directly from Linux kernel interfaces (procfs, sysfs). The frontend
(dashboard.html, gpu.html, presentation.html) is kept as-is but re-wired from
`fetch("/api/…")` to `window.__TAURI__.core.invoke(…)` Tauri IPC calls.

**Scope boundary:** Linux only, AMD GPU focus, single GPU (index 0).

---

## Architecture After Migration

```
src-tauri/
  src/
    main.rs          — Tauri app entry point
    lib.rs           — registers all commands
    metrics/
      mod.rs         — public re-exports
      cpu.rs         — /proc/stat + /proc/cpuinfo
      memory.rs      — /proc/meminfo
      disk.rs        — /proc/diskstats (rate deltas)
      network.rs     — /proc/net/dev (rate deltas)
      processes.rs   — /proc/[pid]/stat + status
      gpu_amd.rs     — /sys/class/drm/card0/device/...
      battery.rs     — /sys/class/power_supply/
      static_info.rs — one-shot CPU/OS/disk/GPU static data
  Cargo.toml
  tauri.conf.json

dashboard.html       — unchanged markup, fetch → invoke
gpu.html             — unchanged markup, fetch → invoke
presentation.html    — unchanged markup, fetch → invoke
```

---

## Sub-Tasks

---

### Sub-Task 1 — Scaffold the Tauri project

**Intent**
Create the `src-tauri/` directory structure and configuration files so the project
compiles as a Tauri app. No metric logic yet — just a working shell that opens a
window showing `dashboard.html`.

**Expected Outcomes**
- `cargo tauri dev` opens a desktop window rendering `dashboard.html`
- `cargo tauri build` produces a `.deb` / AppImage bundle
- `cargo check` inside `src-tauri/` passes with zero errors

**Todo List**
1. Run `npm create tauri-app@latest` (or `cargo tauri init`) inside the repo root to
   generate `src-tauri/` scaffold — answer: identifier `com.dashboard`, window title
   "System Dashboard", `distDir` pointing to repo root (`../`), `devUrl` left empty.
2. Edit `src-tauri/tauri.conf.json`:
   - Set `build.frontendDist` to `"../"` so Tauri serves the HTML files from the repo root.
   - Set the initial window URL to `dashboard.html`.
   - Disable CSP for now (re-enable later once IPC is wired).
3. Add `serde`, `serde_json`, and `tokio` to `src-tauri/Cargo.toml`.
4. Replace the generated stub `main.rs` / `lib.rs` with minimal versions that register
   zero commands but build cleanly.
5. Verify `cargo tauri dev` opens a window with the existing dashboard UI visible.

**Relevant Context**
- Repo root: `/home/markus/src/newdashboard`
- Existing HTML entry: `dashboard.html`
- Tauri v2 uses `tauri::Builder::default().run(...)` in `main.rs`; commands are
  registered in `lib.rs` via `.invoke_handler(tauri::generate_handler![...])`

**Status:** `[x] done`

---

### Sub-Task 2 — Rust metrics module: CPU & Memory

**Intent**
Implement the `/api/stats` CPU and memory fields as Rust Tauri commands reading from
`/proc/stat` and `/proc/meminfo`. Establish the module layout and serde response structs
that all subsequent metrics sub-tasks will follow.

**Expected Outcomes**
- `invoke("get_stats")` returns a JSON object with correct `cpu` and `memory` fields
- `cargo test -p tauri-app` passes unit tests with fake procfs fixtures
- Values match `server.js` field names exactly (no frontend changes needed yet)

**Todo List**
1. Create `src-tauri/src/metrics/mod.rs` with public re-exports.
2. Implement `src-tauri/src/metrics/cpu.rs`:
   - Parse `/proc/stat` to compute per-core load percentages (delta between two reads,
     100 ms apart — same technique systeminformation uses).
   - Return `CpuStats { load_percent, user_percent, sys_percent, cores: Vec<f32> }`.
   - Unit test: write a temp file with two synthetic `/proc/stat` snapshots, assert
     computed percentages match expected values.
3. Implement `src-tauri/src/metrics/memory.rs`:
   - Parse `/proc/meminfo` key-value pairs.
   - Return `MemStats { total_bytes, used_bytes, active_bytes, free_bytes, avail_bytes,
     swap_total, swap_used, used_percent }`.
   - Unit test: fake `/proc/meminfo` content, assert all fields parse correctly.
4. Add a `#[tauri::command] async fn get_stats(...)` stub in `lib.rs` that calls only
   cpu + memory and returns `serde_json::Value` with those two fields (other fields
   stubbed as `null` for now).
5. Register the command in `tauri::generate_handler!`.

**Relevant Context**
- `server.js` lines 48–65: exact field names and rounding rules to match
- `/proc/stat` format: first line is aggregate, subsequent lines are `cpu0`, `cpu1`, …
- `/proc/meminfo`: `MemTotal`, `MemFree`, `MemAvailable`, `Active`, `SwapTotal`,
  `SwapFree` — `used = total - free`, `swap_used = swap_total - swap_free`
- Rounding: one decimal place (`(val * 10.0).round() / 10.0`)

**Note:** Metrics logic extracted into `src-tauri/crates/metrics/` (a separate crate with
no Tauri/webkit2gtk dependency). `cargo check/test -p metrics` runs in ~3–8 s. The `app`
crate imports it via `metrics = { path = "crates/metrics" }`. All subsequent sub-tasks add
modules to `crates/metrics/` — never to `src-tauri/src/` directly.

**Status:** `[x] done`

---

### Sub-Task 3 — Rust metrics module: Disk I/O & Network

**Intent**
Add disk I/O rates and network throughput to `get_stats`, reading from `/proc/diskstats`
and `/proc/net/dev`. Both require stateful delta computation (current − previous reading
divided by elapsed time), which is handled via a `Mutex`-protected state struct in the
Tauri app state.

**Expected Outcomes**
- `invoke("get_stats")` populates `disk` and `net` fields with correct byte/op rates
- State is initialised on first call; first response returns zeros (same behaviour as
  `systeminformation` on first poll)
- Unit tests cover the delta computation arithmetic

**Todo List**
1. Implement `src-tauri/src/metrics/disk.rs`:
   - Parse `/proc/diskstats`; sum reads/writes across all physical block devices
     (skip loop, ram, dm devices).
   - Store previous counters + timestamp in app state; compute `read_bps`, `write_bps`,
     `read_ops`, `write_ops`.
2. Implement `src-tauri/src/metrics/network.rs`:
   - Parse `/proc/net/dev`; skip `lo` interface.
   - Same delta pattern; return `rx_bps`, `tx_bps`, `rx_total`, `tx_total`, `ifaces`.
3. Add a `MetricsState` struct holding previous disk/net snapshots behind `Mutex`.
4. Register `MetricsState` with `tauri::Builder::default().manage(state)`.
5. Update `get_stats` to call disk + network modules and include results.

**Relevant Context**
- `server.js` lines 67–90: field names and filtering rules
- `/proc/diskstats` columns: major, minor, name, reads\_completed, …, sectors\_read,
  …, writes\_completed, …, sectors\_written — sector size is 512 bytes on Linux
- `/proc/net/dev`: two header lines, then `iface: rx_bytes … tx_bytes …`

**Status:** `[x] done`

---

### Sub-Task 4 — Rust metrics module: Processes

**Intent**
Add the top-40-by-CPU process list to `get_stats` by reading `/proc/[pid]/stat` and
`/proc/[pid]/status`. CPU percentage requires the same delta approach as disk/network.

**Expected Outcomes**
- `invoke("get_stats")` returns a `processes` array of up to 40 entries matching the
  field names in `server.js` (`pid`, `name`, `cmd`, `cpu`, `mem_bytes`, `status`,
  `user`)
- Unit tests cover pid stat parsing and CPU delta calculation

**Todo List**
1. Implement `src-tauri/src/metrics/processes.rs`:
   - Enumerate `/proc/[0-9]*/stat` glob.
   - Parse fields: pid (1), name (2, strip parens), state (3), utime (14), stime (15),
     rss (24 — in pages, multiply by page size).
   - Read `/proc/[pid]/status` for `Name` and `Uid` lines; map UID to username via
     `/etc/passwd`.
   - Delta CPU: `(utime+stime − prev) / (elapsed_ticks)`; normalise by CPU count so
     100 % = one full core (same as `ps` / systeminformation).
   - Sort by CPU desc, take top 40.
   - Read `/proc/[pid]/cmdline` (NUL-separated) for the `cmd` field.
2. Store per-pid previous tick counts in `MetricsState`.
3. Update `get_stats` to include the process list.

**Relevant Context**
- `server.js` lines 92–105: field names, `memBytes = memRss * 1024`
- `/proc/[pid]/stat`: space-separated; field 2 (name) may contain spaces — parse by
  finding last `)` before splitting remaining fields
- Clock ticks per second: `libc::sysconf(libc::_SC_CLK_TCK)` or read from
  `/proc/uptime` + `btime`; add `libc` crate dependency

**Status:** `[x] done`

---

### Sub-Task 5 — Rust metrics module: AMD GPU

**Intent**
Implement AMD GPU metric collection in Rust by reading the same sysfs paths that the
`node-gpu` C addon reads. This replaces the native Node addon entirely.

**Expected Outcomes**
- `invoke("get_stats")` returns a populated `gpu` object for the AMD card
- Returns `null` gracefully if no AMD GPU sysfs path is found
- Unit tests cover every sysfs file parse with fake fixture files
- Field names match `server.js` exactly (`utilization_gpu`, `utilization_memory`, etc.)

**Todo List**
1. Implement `src-tauri/src/metrics/gpu_amd.rs`:
   - Detect AMD card: glob `/sys/class/drm/card*/device/vendor`, match `0x1002`.
   - Read each metric from sysfs (use a helper `read_sysfs_u64(path) -> Option<u64>`):
     - `gpu_busy_percent` → `utilization_gpu` (direct %)
     - `mem_info_vram_used` / `mem_info_vram_total` → `memory_used_mb`, `memory_total_mb`,
       `memory_free_mb` (bytes ÷ 1 048 576)
     - `memory_utilization` = used/total × 100
     - `hwmon/hwmon*/temp1_input` → `temperature_c` (millidegrees ÷ 1000)
     - `hwmon/hwmon*/power1_average` → `power_w` (microwatts ÷ 1 000 000)
     - `pp_dpm_sclk` → `core_clock_mhz` (parse line ending with `*`)
     - `pp_dpm_mclk` → `memory_clock_mhz` (parse line ending with `*`)
     - `hwmon/hwmon*/pwm1` → `fan_speed_pct` (value 0–255, scale to 0–100 %)
   - Read `device/product_name` or `device/model` for `name`; vendor string `"AMD"`.
2. Wire into `get_stats`.
3. Unit tests: create a `TempDir` tree mirroring the sysfs structure with known values;
   assert every field parses to the expected output.

**Relevant Context**
- Source of truth: `node-gpu/src/linux/amd_linux.c` — all paths and unit conversions
  are documented there
- `server.js` lines 107–129: rounding and field names

**Status:** `[x] done`

---

### Sub-Task 6 — Rust metrics module: Battery & Static Info

**Intent**
Implement the battery status command and the one-shot static info command (`/api/static`
equivalent: CPU model, OS info, GPU static info, mounted filesystems).

**Expected Outcomes**
- `invoke("get_battery")` returns `{ has_battery, percent, is_charging }` or
  `{ has_battery: false }`
- `invoke("get_static_info")` returns `{ cpu, os, gpu, disks }` matching `server.js`
  field names
- Both commands have unit tests

**Todo List**
1. Implement `src-tauri/src/metrics/battery.rs`:
   - Glob `/sys/class/power_supply/BAT*/` — if none found, return `has_battery: false`.
   - Read `capacity` (percent) and `status` (`Charging` / `Discharging` / `Full`).
2. Implement `src-tauri/src/metrics/static_info.rs`:
   - **CPU:** parse `/proc/cpuinfo` for `model name`, `cpu MHz`, `cpu cores`,
     `siblings`; read `manufacturer` from the model name string.
   - **OS:** read `/etc/os-release` (`NAME`, `VERSION_ID`), `uname -r` (kernel),
     `hostname` via `gethostname`, `uname -m` (arch).
   - **GPU static:** from `gpu_amd.rs` — name, vendor, total VRAM; driver version from
     `drmVersion` is not in sysfs on AMD — read from
     `/sys/class/drm/card0/device/driver/module/version` if present, else omit.
   - **Disks:** parse `/proc/mounts` for mount points, then `statvfs` each for size/used.
3. Register `#[tauri::command] async fn get_static_info(...)` and
   `#[tauri::command] async fn get_battery(...)` in `lib.rs`.

**Relevant Context**
- `server.js` lines 143–176: exact field names
- `dashboard.html` `applyStaticInfo` function (line 698): consumes these fields

**Status:** `[x] done`

---

### Sub-Task 7 — Wire frontend to Tauri IPC

**Intent**
Replace the `fetch("http://localhost:3000/api/…")` calls in the HTML frontend with
`window.__TAURI__.core.invoke(…)` calls so the dashboard uses the Rust backend.
No visual changes — only the data transport layer changes.

**Expected Outcomes**
- Dashboard renders live data when running under `cargo tauri dev`
- The `const API` / `probeAPI` / `fetchStats` pattern is replaced by thin invoke wrappers
- The app works without any Node.js process running

**Todo List**
1. In `dashboard.html`, replace the `const API = 'http://localhost:3000/api'` block and
   the two fetch functions with:
   ```js
   async function fetchStats()      { return await window.__TAURI__.core.invoke('get_stats'); }
   async function fetchStaticInfo() { return await window.__TAURI__.core.invoke('get_static_info'); }
   ```
2. Remove the `apiAvailable` guard — in Tauri the backend is always present.
3. Replace `probeAPI()` call with a direct `fetchStaticInfo()` call on startup.
4. Apply the same substitution to `gpu.html` and `presentation.html` if they contain
   fetch calls.
5. Add `"withGlobalTauri": true` to `tauri.conf.json` → `app` section so
   `window.__TAURI__` is injected automatically.
6. Smoke test via `cargo tauri dev`: confirm all dashboard panels populate with live data.

**Relevant Context**
- `dashboard.html` lines 673–695: the two functions to replace
- Tauri v2 IPC: `window.__TAURI__.core.invoke('command_name', { arg })` returns a Promise
- Field names in JSON responses must match what the JS already expects (snake_case
  vs camelCase — Rust serde can use `#[serde(rename_all = "camelCase")]` on response
  structs to preserve the existing JS field names)

**Status:** `[x] done`

---

### Sub-Task 8 — Packaging & cleanup

**Intent**
Configure the Tauri bundler to produce a distributable Linux package, remove the
now-unused Node.js server files, and document how to build and run the app.

**Expected Outcomes**
- `cargo tauri build` produces a `.deb` and/or AppImage under `src-tauri/target/release/bundle/`
- `server.js`, `package.json`, `node_modules/`, `node-gpu/` are either removed or
  clearly marked as legacy
- `README.md` is updated with Tauri build/run instructions

**Todo List**
1. Set bundle metadata in `tauri.conf.json`: `identifier`, `version`, `targets`
   (`deb`, `appimage`), `icon` (use a placeholder or export one from the existing
   dashboard favicon if present).
2. Confirm required system libraries for the `.deb` are listed in `externalBin` or
   `debian.depends` (typically `libwebkit2gtk-4.1`, `libgtk-3`).
3. Run `cargo tauri build` and verify the bundle installs and launches.
4. Delete `server.js`, `package.json`, `package-lock.json`, `node_modules/`, and
   `node-gpu/` (including its nested `.git` — `rm -rf node-gpu/`).
5. Update `README.md`: prerequisites (`rustup`, `tauri-cli`), build command, run command.

**Relevant Context**
- Tauri v2 bundle config lives under `tauri.conf.json` → `bundle`
- AppImage requires `squashfs-tools` and `fuse` on the build host

**Status:** `[ ] pending`

---

## Key Decisions & Constraints

| Decision | Choice |
|---|---|
| GPU vendor support | AMD only (sysfs), no NVML/Intel |
| Platform | Linux only |
| Metric backend | Pure Rust, std + serde + tokio + libc |
| Frontend | Unchanged HTML/JS, only transport layer swapped |
| serde naming | `#[serde(rename_all = "camelCase")]` to preserve existing JS field names without any frontend edits |
| Testing | `cargo test` with fake sysfs TempDir fixtures; frontend via dev server + Chrome DevTools MCP |
| Node.js | Eliminated entirely after migration |
| node-gpu | Deleted outright (`rm -rf node-gpu/`) — has its own nested `.git`, not archived |
