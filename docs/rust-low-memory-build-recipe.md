# Rust Low-Memory Build Recipe

Lessons learned reducing peak RSS from 55 GB → ~2 GB for a Tauri project on Linux.
Applicable to any Rust project with heavy C-binding dependencies (GTK, webkit2gtk,
OpenSSL, LLVM, etc.).

---

## The Real Cause of Rust OOMs

General advice about `codegen-units` and `opt-level` **does not apply to C-binding
crates**. The culprit is a different mechanism entirely:

| Myth | Reality |
|---|---|
| "More `codegen-units` = more parallelism = faster" | Each unit spawns a separate LLVM instance. 256 units × 400 MB LLVM = 100 GB |
| "`opt-level = 3` for deps makes tests faster" | `-O3` triples per-unit RSS; LLVM runs full inlining/vectorisation passes on every unit |
| "Splitting into smaller files fixes it" | Pre-generated FFI binding files (e.g. `webkit2gtk-sys/src/lib.rs` = 255 KB) are a **single translation unit** — LLVM holds the whole thing regardless |

**`webkit2gtk-sys/src/lib.rs` was observed consuming 55 GB in a single `rustc` process.**
No `Cargo.toml` knob can split a single source file's IR across LLVM instances.

---

## Fix 1 — Isolate heavy dependencies into a separate crate (most impactful)

Create a Cargo workspace with two crates:

```
my-tauri-project/
├── Cargo.toml          ← workspace root  +  app crate (has Tauri/GTK dep)
└── crates/
    └── mylogic/        ← pure Rust, zero Tauri/GTK deps
        ├── Cargo.toml
        └── src/
```

- `cargo check -p mylogic` and `cargo test -p mylogic` **never touch webkit2gtk**.
- The `app` crate only needs to be compiled for `cargo tauri dev` / `cargo tauri build`.
- All business logic, metric collection, parsing, etc. lives in `mylogic`.

**Result:** check/test go from OOM → 8 s / 3 s respectively.

---

## Fix 2 — Trim tokio features

`tokio = { features = ["full"] }` pulls in `net`, `process`, `signal`, `fs`,
`io-std`, etc. For Tauri commands you only need:

```toml
tokio = { version = "1", features = ["rt", "rt-multi-thread", "time", "macros"] }
```

---

## Fix 3 — Correct `codegen-units` and `opt-level` values for Tauri

These apply to the **full** `app` crate build (`cargo tauri dev/build`). They reduce
*additional* parallelism on top of the unavoidable webkit2gtk cost.

```toml
[profile.dev]
opt-level       = 0
codegen-units   = 4              # do NOT set > 8 for Tauri; each LLVM instance = ~1-2 GB
debug           = "line-tables-only"   # 20% smaller binaries vs full DWARF
split-debuginfo = "unpacked"

[profile.dev.package."*"]
opt-level     = 1                # not 3 — LLVM does far less work, much lower RSS
codegen-units = 4

[profile.test]
inherits = "dev"
```

---

## Fix 4 — Cap parallel `rustc` jobs

```toml
# src-tauri/.cargo/config.toml
[build]
jobs = 8    # 8 parallel rustc × ~2 GB each ≈ 16 GB peak; tune to (free_ram_GB / 2)
```

Formula: `jobs = floor(available_ram_GB / 2)`, capped at CPU core count.

---

## Fix 5 — Use `mold` linker instead of `lld` or `ld`

```toml
# src-tauri/.cargo/config.toml
[target.x86_64-unknown-linux-gnu]
linker    = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

Install: `sudo dnf install mold` / `sudo apt install mold`

`mold` uses parallel section merging and is measurably faster than `lld` and `ld`
on Linux for large dependency trees.

---

## Fix 6 — `sccache` for cross-build caching

```toml
# src-tauri/.cargo/config.toml
[build]
rustc-wrapper = "/home/user/.cargo/bin/sccache"
```

Install: `cargo install sccache --locked`

After a cold build, subsequent clean builds of unchanged deps are cache hits (seconds,
not minutes). Especially valuable for CI or after `cargo clean`.

---

## Fix 7 — `cargo-nextest` for faster test execution

```bash
cargo install cargo-nextest --locked
cargo nextest run -p mylogic   # parallel process-per-test runner, up to 3× faster
```

---

## Summary Table

| Fix | Addresses | Impact |
|---|---|---|
| **Workspace split** (Fix 1) | webkit2gtk/WRY in test path | ✅ Eliminates OOM entirely for check/test |
| **Trim tokio features** (Fix 2) | unnecessary dep compilation | 🟡 Moderate — fewer crates to compile |
| **codegen-units = 4** (Fix 3) | excess LLVM parallelism | 🟡 Moderate — reduces additive RSS |
| **opt-level = 1 for deps** (Fix 3) | LLVM optimisation work | 🟡 Moderate — 2-3× less LLVM RSS per unit |
| **jobs = 8** (Fix 4) | parallel rustc count | 🟡 Moderate — hard ceiling on peak RSS |
| **mold linker** (Fix 5) | link time | 🟢 Fast — saves 20-40% link time |
| **sccache** (Fix 6) | repeated builds | 🟢 Fast — cache hits skip rustc entirely |
| **nextest** (Fix 7) | test execution speed | 🟢 Fast — 2-3× faster test runs |

---

## Quick diagnosis checklist

If `cargo check` or `cargo test` OOMs on your project:

1. **Identify the offending crate:**
   `ps aux | grep rustc` while building — note the file path in the args.
   Look for `*-sys` crates with large `src/lib.rs` files (> 50 KB).

2. **Check if it's a binding crate:**
   `find ~/.cargo/registry/src -path "*/THE-CRATE-*/src/lib.rs" | xargs wc -l`
   > 5 000 lines = pre-generated C bindings = workspace-split is the only fix.

3. **Check your `codegen-units`:**
   Never set > 16 for projects with heavy C-binding deps. Default (256 in older
   configs) is lethal.

4. **Check `opt-level` for deps:**
   `opt-level = 3` for deps during dev/test is almost never worth the RSS cost.
   Use `opt-level = 1`.
