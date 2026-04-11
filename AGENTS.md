# AGENTS.md

## Project Overview

ESP32-C6 (`no_std` + `alloc`) hosts a Wasmi WebAssembly interpreter that runs a guest WASM program to drive a 16×16
WS2812B LED matrix over GPIO10. Communication with external services uses MQTT over WiFi (Embassy async runtime).

## Architecture

Five workspace crates with a strict dependency direction — **guest → common ← host-common ← host-esp32c6**:

| Crate          | Target                         | Role                                                                                    |
|----------------|--------------------------------|-----------------------------------------------------------------------------------------|
| `common`       | any (`no_std`)                 | Shared constants (panel dimensions)                                                     |
| `guest`        | `wasm32-unknown-unknown`       | WASM program exporting `init()` and `update(ticks, frame, host_buffer_offset) → offset` |
| `host-common`  | any (`no_std`)                 | Reusable host-side logic (e.g. serpentine index mapping) — only crate with unit tests   |
| `host-esp32c6` | `riscv32imac-unknown-none-elf` | Embedded app: loads guest WASM, runs LED driver, WiFi, MQTT                             |
| `dummy`        | native                         | Placeholder so `cargo check` works without cross-compilation targets                    |

### Data Flow (frame pipeline)

1. `wasm_task` calls guest `update()` → guest writes RGB pixels into WASM linear memory and returns a buffer offset.
2. `wasm_task` publishes the raw pointer via atomics (`FRAME_PTR`/`FRAME_LEN`) and signals `FRAME_READY`.
3. `led_task` reads the pixel slice, applies serpentine remapping + gamma + brightness, writes to the LED strip via RMT,
   then signals `FRAME_CONSUMED`.

This is a lockstep producer/consumer — `wasm_task` blocks on `FRAME_CONSUMED` before the next frame.

### Guest ↔ Host Contract

- Guest is a `cdylib` (`#![no_std]`, `#![no_main]`).
- Must export: `init()`, `update(ticks: u64, frame: u64, host_buffer_offset: u32) -> u32`.
- The returned `u32` is a byte offset into WASM linear memory pointing to a `768`-byte RGB buffer (16×16×3).
- Guest may use its own static buffers or write into the host-provided buffer at `host_buffer_offset`.
- Timing unit: **256 ticks per second** (converted from wall-clock on the host side).

## Build & Run

**Prerequisites:** `just`, Rust stable toolchain (targets `riscv32imac-unknown-none-elf` + `wasm32-unknown-unknown`
installed via `rust-toolchain.toml`), ImageMagick `convert` for asset processing, `espflash` for flashing.

```sh
just build          # Build guest WASM then host firmware (runs both sub-justfiles)
just run            # Build + flash + run on physical ESP32-C6
just test           # Unit tests (host-common only)
just ci             # Clippy + fmt check for guest and host
```

Guest and host have **separate justfiles** with independent build pipelines:

- `guest/justfile`: converts assets (PNG/GIF → raw RGB via ImageMagick), then
  `cargo build --release --target=wasm32-unknown-unknown`
- `host-esp32c6/justfile`: `cargo build --release --target=riscv32imac-unknown-none-elf`

The host binary **embeds the guest WASM** at compile time via
`include_bytes!("../../target/wasm32-unknown-unknown/release/guest.wasm")` in `host-esp32c6/src/wasm.rs` — so the guest
**must** be built before the host.

### Environment Variables

`WIFI_SSID` and `WIFI_PASSWORD` are required at **compile time** (`env!()` in `host-esp32c6/src/net.rs`). Set them
before building the host, or it will fail.

### Simulation

A Wokwi config (`host-esp32c6/wokwi.toml` + `diagram.json`) is provided for simulation without hardware.

## Conventions

- **Edition 2024** across all crates, resolver `"3"`.
- `#![no_std]` everywhere except `dummy`. Crates that have tests use `#![cfg_attr(not(test), no_std)]` to enable `std`
  in test mode.
- Logging via the `log!` macro in `host-esp32c6/src/lib.rs` — dual-outputs to `defmt` (RTT) and `esp_println` (UART).
  Use `defmt::Debug2Format` to print non-defmt types.
- Async tasks use `#[embassy_executor::task]` and live in dedicated modules (`wasm.rs`, `led.rs`, `mqtt.rs`, `net.rs`).
- Guest animation assets live in `guest/assets/`; the build script (`guest/build.rs`) parses Aseprite JSON to generate
  frame-offset lookup tables included at compile time.
- `default-members` in workspace `Cargo.toml` excludes `guest` and `host-esp32c6` so plain `cargo check`/`cargo build`
  works on any host without cross-compilation.
- Clippy is run with `-D warnings` in CI for both guest and host targets.

## Key Files

- `host-esp32c6/src/bin/main.rs` — Embassy entrypoint, spawns all async tasks
- `host-esp32c6/src/wasm.rs` — Wasmi engine setup + frame loop
- `host-esp32c6/src/led.rs` — LED strip driver (RMT + serpentine mapping)
- `guest/src/lib.rs` — All guest animations and the `update()` dispatch
- `guest/build.rs` — Aseprite JSON → Rust code generation
- `common/src/lib.rs` — Panel dimension constants shared by guest and host

