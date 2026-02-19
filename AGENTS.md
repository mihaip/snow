# Snow Emscripten Port - Agent Guide

This document captures knowledge for AI agents working on the Snow emulator's web port.

## Project Goal

Snow is a Rust-based Classic Macintosh emulator. This is a fork of it, with the goal of compiling it to WebAssembly via Emscripten so it can run in the browser as part of the [Infinite Mac](https://infinitemac.org) project.

The web version lives in the `frontend_im` crate (im = Infinite Mac). The emulator core in `snow_core` is shared between native and web builds.

## Your Role

In addition to helping the user port the Snow emulator to be web, keep in mind that the user is learning Rust. Specifically:

- Be somewhat skeptical of non-idiomatic Rust suggestions they make; validate against Rust norms.
- Explain Rust-specific constructs (operators not common in other languages, etc.) in more detail.

## Direectory Structure

This repo is a Git submodule of the larger Infinite Mac project repo. Its parent directory is thus the `infinite-mac` root, and many changes need to span across both this Snow submodule and the Infinite Mac repo.

## Build Commands

### Emscripten Build (via Docker)

From the parent `infinite-mac` root directory:

```bash
# Fast iteration: check compilation without linking
scripts/docker-shell.sh -c "source /emsdk/emsdk_env.sh && cd /snow && cargo check -p snow_frontend_im --target wasm32-unknown-emscripten"

# Debug build: no LTO, faster compile (~33s)
scripts/docker-shell.sh -c "source /emsdk/emsdk_env.sh && cd /snow && cargo build -p snow_frontend_im --target wasm32-unknown-emscripten"

# Release build: with LTO, optimized output (~38s)
scripts/docker-shell.sh -c "source /emsdk/emsdk_env.sh && cd /snow && cargo build -r -p snow_frontend_im --target wasm32-unknown-emscripten"

# Interactive shell for debugging
scripts/docker-shell.sh
# Then inside:
source /emsdk/emsdk_env.sh
cd /snow
cargo build -r -p snow_frontend_im --target wasm32-unknown-emscripten
```

Build outputs appear in `snow/target/wasm32-unknown-emscripten/release/` (or `debug/`):

- `snow.js` - JavaScript module
- `snow.wasm` - WebAssembly binary
- `snow.wasm.map` - Source map

See `.cargo/config.toml` for Emscripten-specific flags.

### Import into Infinite Mac

From the parent `infinite-mac` root directory:

```bash
scripts/import-emulator.sh snow
```

## Cross-Platform Development Patterns

The repo is a fork of the upstream Snow repository, but the goal is to minimize divergence so that it can be frequently rebased (and any general improvements can be upstreamed). Keep that in mind when making changes required to make things wonder under Emscripten.

### 1. Target-Specific Dependencies (Cargo.toml)

For crates that don't compile on Emscripten, use target-specific dependencies:

```toml
# Always available
[dependencies]
serde = "1.0"

# Only on non-Emscripten platforms
[target.'cfg(not(target_os = "emscripten"))'.dependencies]
socket2 = { version = "0.6", features = ["all"] }

# Only on Unix (excluding Emscripten)
[target.'cfg(all(unix, not(target_os = "emscripten")))'.dependencies]
nix = { version = "0.29", features = ["term", "fs"] }
```

### 2. Conditional Module Compilation

For modules that won't compile on Emscripten:

```rust
#[cfg(not(target_os = "emscripten"))]
pub mod localtalk_bridge;
```

### 3. File Substitution with #[path]

When a module needs different implementations per platform, use `#[cfg_attr]` with `#[path]` to swap entire files:

```rust
#[cfg_attr(not(target_os = "emscripten"), path = "serial_bridge.rs")]
#[cfg_attr(target_os = "emscripten", path = "serial_bridge_emscripten.rs")]
pub mod serial_bridge;
```

The stub file (`serial_bridge_emscripten.rs`) must export the same public types with compatible APIs. Methods can return errors or no-ops since they won't be called.

**This pattern is preferred** over scattered `#[cfg]` guards because:

- Keeps the original file unchanged (easier to rebase from upstream)
- All platform-specific code is isolated in one place
- Cleaner than many small cfg blocks throughout the codebase

### 4. Backend Traits + Target Injection

When a device's behavior is shared but the backend varies per platform, keep the protocol logic in `snow_core` and expose a small backend trait there. Implement the backend in the platform crate (e.g., `frontend_im`) and attach it through a generic target hook. This keeps platform bindings (JS, OS APIs) out of core and minimizes divergence.

Example shape:

- Core defines a `DiskImage` trait and a `ScsiTargetDisk` wrapper.
- Frontend implements the backend and passes it through the disk image attachment API.
- Attach with `attach_disk_image_at` to keep platform bindings out of core.

### 5. Build After Changes

Always make sure the relevant targets build after code changes. For web work, at minimum run:

```bash
scripts/docker-shell.sh -c "source /emsdk/emsdk_env.sh && cd /snow && cargo check -p snow_frontend_im --target wasm32-unknown-emscripten"
```

### 6. Worker API Source (Emscripten JS glue)

The worker-side API that Emscripten code calls (`workerApi.*`) is defined in
`src/emulator/worker/worker.ts` (see the `EmulatorWorkerApi` class), not in the
generated JS output. When adding new cross-boundary calls from Rust or C/C++,
expose them via `snow/frontend_im/src/js_api/exports.js`, then wrap the
corresponding `extern "C"` bindings in a safe Rust wrapper inside
`snow/frontend_im/src/js_api/`. All other Rust code should call these safe
wrappers (e.g., `js_api::audio::enqueue`) rather than `unsafe` functions.

If you cannot run a build, say so explicitly and explain why.

### 7. Frontend Logging Style

The `snow_frontend_im` binary initializes `env_logger` with a trace filter in
`snow/frontend_im/src/main.rs`, so prefer `log::info!` (or `log::warn!`/`log::error!`)
for visibility during debugging. Avoid `log::debug!` unless you also update the
logger configuration.

## File Structure

```
snow/
├── core/                    # Main emulator library (snow_core)
│   ├── src/
│   │   ├── mac/
│   │   │   ├── serial_bridge.rs            # Native implementation
│   │   │   ├── serial_bridge_emscripten.rs # Web stub
│   │   │   ├── scsi/
│   │   │   │   ├── disk.rs                 # ScsiTargetDisk wrapper and disk target logic
│   │   │   │   ├── disk_image.rs           # DiskImage trait + FileDiskImage
│   │   │   │   ├── target.rs               # ScsiTarget trait for generic attachment
│   │   │   │   ├── controller.rs           # attach_disk_image_at wiring
│   │   │   └── mod.rs                      # Uses #[path] to select
│   │   └── ...
│   └── Cargo.toml
├── frontend_im/              # Infinite Mac frontend (web)
│   ├── src/main.rs           # CLI entry point and wiring for the web build
│   ├── src/disk.rs           # JS-backed disk image backend for SCSI targets
│   ├── src/framebuffer.rs    # Framebuffer bridge for video output
│   ├── src/js_api/           # Safe Rust wrappers around JS/worker APIs
│   │   ├── exports.js        # Emscripten JS glue exports
│   └── Cargo.toml
├── .cargo/
│   └── config.toml           # Emscripten linker flags
└── AGENTS.md                 # This file
```
