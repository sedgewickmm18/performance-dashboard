
### Showcase for vibe coding with Chrome-dev MCP


Install with

```
npm i
```

and start with

```
PORT=3000 node server.js
```


Process list shows fake data, all other data is pulled live from the local system.


### Setting up your code agent for chrome-dev MCP

Let your coding agent, for example IBM Bob, set it up for you with the following [skill](https://raw.githubusercontent.com/sedgewickmm18/performance-dashboard/refs/heads/master/chrome-devtools-setup/SKILL.md) `./chrome-dev-MCP/SKILL.md`. The only step remaining is to start chrome in debug mode with

```bash
/opt/google/chrome/chrome \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/chrome-debug \
  --no-first-run \
  --disable-extensions \
  "http://localhost:3000"   # replace with your dev server URL
```

---

## Tauri desktop application

A fully self-contained native desktop variant of the dashboard (Linux only).
No Node.js, no Express server — all metrics are collected directly from the Linux
kernel by a Rust backend and delivered to the frontend via Tauri IPC (`invoke`).

### What is collected

| Metric | Linux source |
|---|---|
| CPU load (total + per-core) | `/proc/stat` |
| Memory & swap | `/proc/meminfo` |
| Disk I/O rates | `/proc/diskstats` |
| Network throughput | `/proc/net/dev` |
| Top-40 processes by CPU | `/proc/[pid]/stat` + `/proc/[pid]/cmdline` |
| AMD GPU (utilisation, VRAM, temp, power, clocks, fan) | `/sys/class/drm/card*/device/` |
| Battery | `/sys/class/power_supply/BAT*/` |
| Static info (CPU model, OS, mounted disks) | `/proc/cpuinfo`, `/etc/os-release`, `statvfs` |

### Prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Tauri CLI
cargo install tauri-cli --locked

# System libraries (Fedora / RHEL)
sudo dnf install webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel

# System libraries (Debian / Ubuntu)
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev

# Optional but recommended — faster linker and build cache
sudo dnf install mold          # or: sudo apt install mold
cargo install sccache --locked
cargo install cargo-nextest --locked
```

### Run in development mode

Opens the dashboard in a native window with live data:

```bash
./build-test.sh dev
# or directly:
cd src-tauri && cargo tauri dev
```

> **First run** compiles the full Tauri dependency tree (webkit2gtk, WRY, GTK) —
> expect **~10 minutes** on a cold cache.
> Subsequent runs are incremental and start in seconds thanks to `sccache` and `mold`.

### Build a distributable package

```bash
./build-test.sh release
# or directly:
cd src-tauri && cargo tauri build
```

Produces a `.deb` installer and an AppImage under:

```
src-tauri/target/release/bundle/
├── deb/       system-dashboard_*.deb
└── appimage/  system-dashboard_*.AppImage
```

### Run tests (fast — no webkit2gtk required)

All metric logic lives in `src-tauri/crates/metrics/` — a standalone crate with
zero GTK/Tauri dependencies. Checks and tests complete in seconds:

```bash
./build-test.sh          # run all 37 unit tests  (~3 s)
./build-test.sh check    # type-check only         (~8 s)
```

Or directly with Cargo:

```bash
cd src-tauri
cargo nextest run -p metrics
```

### Architecture

```
src-tauri/
├── src/lib.rs                   Tauri command layer (get_stats, get_static_info, get_battery)
└── crates/metrics/src/metrics/
    ├── cpu.rs                   /proc/stat
    ├── memory.rs                /proc/meminfo
    ├── disk.rs                  /proc/diskstats
    ├── network.rs               /proc/net/dev
    ├── processes.rs             /proc/[pid]/stat
    ├── gpu_amd.rs               /sys/class/drm/card*/device/
    ├── battery.rs               /sys/class/power_supply/BAT*/
    └── static_info.rs           /proc/cpuinfo, /etc/os-release, statvfs
```

