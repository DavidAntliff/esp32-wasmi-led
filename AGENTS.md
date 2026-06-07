# AGENTS.md

## Project Overview

ESP32-C6 (`no_std` + `alloc`) hosts a [Wasmi](https://github.com/wasmi-labs/wasmi) WebAssembly
interpreter that runs a guest WASM program to drive a 16×16 WS2812B LED matrix over GPIO10.
The device connects to WiFi and is controlled over MQTT (Embassy async runtime). This repo is the
**device tier** of a planned three-tier system — see [Roadmap](#roadmap--wider-system-architecture).

## Hardware

- **MCU**: ESP32-C6 (RISC-V `riscv32imac-unknown-none-elf`) with built-in USB-JTAG (flash over USB, no
  external probe needed).
- **USB port**: connect the cable to the **right-most port, labelled "USB"** on the board (the native
  USB-JTAG/serial). This is the one probe-rs uses for flashing and defmt/serial output — the other port
  (UART bridge) will not work for `just run`/serial comms.
- **LEDs**: 256× WS2812B in a **16×16 serpentine** grid on **GPIO10**, driven by the **RMT** peripheral
  at 80 MHz in **GRB** color order.
- **Serpentine layout** (`host-common/src/lib.rs::serpentine_index`): strip LED 0 is the **bottom-left**
  corner; even physical rows run left→right, odd rows right→left; the **top-left** is the last LED (255).
  The framebuffer origin `(0,0)` is **top-left** and is flipped to physical bottom-left during mapping.
- LED output applies `gamma` correction and a global `brightness` of `100/255` (`host-esp32c6/src/led.rs`).

## Architecture

Five workspace crates (`Cargo.toml`, resolver `"3"`, edition 2024). Dependencies:
`guest → common`; `host-esp32c6 → common + host-common`; `common`, `host-common`, `dummy` are standalone.

| Crate          | Target                         | Role                                                                                    |
|----------------|--------------------------------|-----------------------------------------------------------------------------------------|
| `common`       | any (`no_std`)                 | Shared constants (16×16, `LED_BUFFER_SIZE = 768`) + unsafe pixel helpers (`set_color`, `set_all`) |
| `guest`        | `wasm32-unknown-unknown`       | `cdylib` WASM program exporting `init()` and `update(ticks, frame, host_buffer_offset) → offset` |
| `host-common`  | any (`no_std`)                 | Reusable host-side logic (`serpentine_index`) — the **only crate with unit tests**      |
| `host-esp32c6` | `riscv32imac-unknown-none-elf` | Embedded app: loads guest WASM, runs LED driver, WiFi, MQTT, Direct mode                |
| `dummy`        | native                         | Placeholder so `cargo check` works without cross-compilation targets                    |

### Tasks (Embassy)

`host-esp32c6/src/bin/main.rs` spawns: `connection`/`net_task` (WiFi + net stack), `mqtt_task`,
`wasm_task`, `direct_task`, `led_task`. Each lives in its own module.

### Data Flow (frame pipeline)

1. The active producer (`wasm_task` or `direct_task`) writes RGB pixels into a 768-byte buffer and
   publishes the raw pointer/length via atomics (`FRAME_PTR`/`FRAME_LEN`), then signals `FRAME_READY`.
2. `led_task` reads the pixel slice, applies serpentine remap + gamma + brightness, writes to the strip
   via RMT (inside a `critical_section`), then signals `FRAME_CONSUMED`.
3. The producer blocks on `FRAME_CONSUMED` before the next frame — a strict lockstep producer/consumer,
   so the LED task can safely read the producer's memory without copying.

### Modes

`MODE` (an Embassy `Watch`) broadcasts the current `Mode` to all tasks:

- **`Wasm`** (default): `wasm_task` calls the guest `update()` every ~1 ms and produces frames.
- **`Direct`**: `wasm_task` idles; `direct_task` applies `DirectCommand`s (from the `DIRECT_CMD` channel,
  fed by MQTT) straight into the host pixel buffer and produces frames.

### Guest ↔ Host Contract

- Guest is a `cdylib` (`#![no_std]`, `#![no_main]`) with its own panic handler.
- Must export: `init()`, `update(ticks: u64, frame: u64, host_buffer_offset: u32) -> u32`.
- The returned `u32` is a byte offset into WASM linear memory pointing to a `768`-byte RGB buffer
  (16×16×3). The guest may write into the **host-provided buffer** at `host_buffer_offset` (return that
  offset) **or** use its own static buffer (return that buffer's pointer cast to `u32`).
- At startup the host grows guest memory by 1 page (64 KiB) and hands the guest a buffer at the end of
  its linear memory; the pointer is also published in `HOST_BUFFER_PTR` for `direct_task`.
- Timing unit: **256 ticks per second** (host converts wall-clock elapsed ms → ticks).

## MQTT Control Protocol

Configured in `host-esp32c6/src/mqtt.rs` (MQTT v5 via `rust-mqtt`):

- **Broker**: `192.168.1.201:1883`, user `testUser` / pass `testPass`, client id `rust-mqtt-demo-client`,
  last-will on topic `i/am/dead`.
- **Subscribes** to `host-esp32c6/mbox` — inbound control commands (JSON `Command`, parsed with
  `serde_json_core`).
- **Publishes** status to `test` (a hello message, then `Update #N from host-esp32c6` every 5 s).
- **The broker must be reachable at `BROKER_IP` when the device boots.** If nothing is listening there,
  `mqtt_task` logs `TCP connect failed` / `ConnectionReset` and **exits — it does not retry until the
  device reboots**, so bring the broker up first. `just mosquitto` runs one locally (see the
  **mqtt-control** skill).

`Command` / `DirectCommand` are externally-tagged serde enums (`host-esp32c6/src/lib.rs`). `Point`
(`{x,y}`, top-left origin) and `Rgb` (`{r,g,b}`) are `u8`. Valid payloads:

```json
{"SetMode":"Wasm"}
{"SetMode":"Direct"}
{"DirectCommand":{"SetPixel":{"point":{"x":5,"y":3},"color":{"r":255,"g":0,"b":0}}}}
{"DirectCommand":{"SetAll":{"color":{"r":0,"g":255,"b":0}}}}
```

## Network Configuration (must match your LAN)

Several values are **hardcoded** and must be edited before building for a different network:

- **MQTT broker IP** — `BROKER_IP` in `host-esp32c6/src/mqtt.rs` (`192.168.1.201`).
- **Device static IP / gateway / DNS** — `host-esp32c6/src/bin/main.rs` (`192.168.1.242/24`, gw/dns
  `192.168.1.1`). A DHCP alternative is commented out alongside.
- **WiFi credentials** — `WIFI_SSID` / `WIFI_PASSWORD` (see the secrets note below).

### WiFi credentials & secrets

`WIFI_SSID` and `WIFI_PASSWORD` are read via `env!()` at **compile time** (`host-esp32c6/src/net.rs`),
so they must be present in the environment when the host crate is built — the build fails with
`environment variable not defined` otherwise.

They are supplied from a local **`.envrc`** file (loaded automatically by [direnv](https://direnv.net/),
or `source .envrc` manually) that `export`s the two variables into the shell. This file holds real
network credentials and is intentionally **git-ignored** (see `.gitignore`).

> **🔒 Never commit these values.** `.envrc` (and the SSID/password it contains) must never be added to
> a commit, hardcoded into source, or written into documentation. Keep them in the git-ignored `.envrc`
> only; reference the variable names (`$WIFI_SSID`, `$WIFI_PASSWORD`) in commands so the literal values
> never appear in the repo, command history, or logs. If you add or change how these are loaded, keep
> the file git-ignored.

## Build & Run

**Prerequisites:**

- `just` — task runner (`cargo install just`).
- Rust **stable** — `rust-toolchain.toml` auto-installs the `rust-src` component and both targets
  (`riscv32imac-unknown-none-elf`, `wasm32-unknown-unknown`). `build-std` requires `rust-src`.
- **`probe-rs`** — flashing/running on hardware (`cargo install probe-rs-tools`). The `cargo run` runner
  is `probe-rs run --chip=esp32c6 --probe 303a:1001 …` (`303a:1001` is the ESP32-C6 USB-JTAG).
- **ImageMagick** (`convert`) — guest asset conversion (PNG/GIF → raw RGB).
- **Docker** — for the local Mosquitto MQTT broker.
- *Optional:* `wasm-opt` / `wasm-tools` (guest `opt`/`print`), `feh` + ImageMagick `montage` (guest
  `gallery`).

**Getting everything working from a clean checkout:**

```sh
# 1. Toolchain (auto-installed from rust-toolchain.toml; manual fallback shown)
rustup component add rust-src
rustup target add riscv32imac-unknown-none-elf wasm32-unknown-unknown
cargo install just probe-rs-tools

# 2. Edit hardcoded network values to match your LAN (see Network Configuration above):
#      host-esp32c6/src/mqtt.rs      -> BROKER_IP
#      host-esp32c6/src/bin/main.rs  -> static IP / gateway / DNS

# 3. WiFi credentials (compile-time, required — host build fails without them).
#    Put these in a git-ignored .envrc (loaded by direnv, or `source .envrc`).
#    NEVER commit real values — see "WiFi credentials & secrets" above.
export WIFI_SSID="…"        # in .envrc, not committed
export WIFI_PASSWORD="…"    # in .envrc, not committed

# 4. Build, test, lint
just build          # guest assets -> guest WASM -> host firmware (correct order)
just test           # unit tests (host-common only)
just ci             # clippy (both targets) + fmt check

# 5. Flash + run on a USB-connected ESP32-C6 (via probe-rs)
just run

# 6. Local MQTT broker (separate terminals)
just mosquitto          # Mosquitto in Docker on :1883
just mosquitto-monitor  # subscribe to all topics ('#')
```

Guest and host have **separate justfiles** with independent pipelines:

- `guest/justfile`: `build-assets` converts assets via ImageMagick (`convert … rgb:target/*.raw`;
  GIFs use `-coalesce` + `%03d` per frame), then `cargo build --release --target=wasm32-unknown-unknown`.
- `host-esp32c6/justfile`: `cargo build --release --target=riscv32imac-unknown-none-elf` (`build`) /
  `cargo run …` (`run`, flashes via probe-rs).

**Build order matters.** The guest's `build` depends on `build-assets` (the raw files must exist for
`include_bytes!`), and the host **embeds the built guest WASM** at compile time via
`include_bytes!("../../target/wasm32-unknown-unknown/release/guest.wasm")` (`host-esp32c6/src/wasm.rs`).
So the guest must be fully built before the host. `just build` handles this ordering.

### Simulation

A Wokwi config (`host-esp32c6/wokwi.toml` + `diagram.json`) is provided to simulate the board + LED
matrix without hardware.

## Guest Animations & Asset Pipeline

`guest/src/lib.rs::update()` dispatches by tick (256 ticks/sec): the first 512 ticks run a boot test
(`white` → `corners`), then it cycles `ticks % 4096` across `rainbow_cycle` (0–1024), `proc0001`
(1024–2048), `anim0001` (2048–3072), and `anim0002` (3072–4096).

To add an animation:

1. Drop the source asset in `guest/assets/`.
2. Add a `convert` line to `build-assets` in `guest/justfile` producing raw RGB in `guest/target/`
   (static image → single `.raw`; GIF → `…_%03d.raw` per frame; Aseprite sprite-sheet → one `.raw`
   plus an exported JSON for `build.rs`).
3. For Aseprite sprite sheets, `guest/build.rs` parses the JSON metadata and generates a `FRAME_OFFSETS`
   table (durations converted ms → ticks) in `OUT_DIR`.
4. In `guest/src/lib.rs`, `include_bytes!` the raw data and write an animation fn returning a buffer
   offset/pointer, then wire it into the `update()` dispatch (`ticks % 4096` arms).
5. Rebuild the guest (then the host) — `just build`.

## Conventions

- **Edition 2024** across all crates, resolver `"3"`.
- `#![no_std]` everywhere except `dummy`. `common` and `host-common` use
  `#![cfg_attr(not(test), no_std)]` so they can build with `std` under test; only `host-common` actually
  has unit tests.
- Logging via the `log!` macro in `host-esp32c6/src/lib.rs` — dual-outputs to `defmt` (RTT) and
  `esp_println` (UART). Use `defmt::Debug2Format` / `Display2Format` for non-`defmt` types. defmt log
  *locations* may render as `<invalid location: defmt frame-index: N>` under probe-rs — this is cosmetic
  (a frame-index/decoder mismatch); the log **messages** decode correctly.
- Async tasks use `#[embassy_executor::task]` and live in dedicated modules (`wasm.rs`, `led.rs`,
  `mqtt.rs`, `net.rs`, `direct.rs`).
- Inter-task sync: `Signal` (frame handshake), `AtomicUsize` (pointer sharing, Release/Acquire),
  `Watch` (mode broadcast), `Channel` (`DIRECT_CMD`).
- `default-members` in workspace `Cargo.toml` is `dummy` + `host-common`, so a bare `cargo check` /
  `cargo build` runs natively without cross-compilation. Specify `-p`/`--target` for the other crates.
- Clippy is run with `-D warnings` in CI for both the guest and host targets; there is **no GitHub CI**
  (`.github/` is empty) — CI is the local `just ci`.

## Roadmap / Wider System Architecture

This repo is the **device** tier of a planned three-tier system:

```
browser (egui/WASM) ──WS/HTTP──► axum backend ──MQTT──► ESP32-C6 (this repo)
```

The sibling repo **`../egui-axum-mqtt-demo`** (checked out alongside this one) is the reference template
for the front+back tiers, demonstrating the patterns to bring in:

- **Frontend**: `eframe`/`egui` 0.33 compiled to WASM, served by `trunk` (dev port 8080); talks to the
  backend over WebSocket (realtime) + HTTP GET (poll) via `gloo-net`.
- **Backend**: `axum` 0.8 on `0.0.0.0:3000`, bridging WebSocket ↔ MQTT with `rumqttc` 0.25; serves the
  built WASM from `backend/dist/` and broadcasts MQTT → all WS clients via `tokio::broadcast`.
- **Common**: shared `serde` message types (`ClientMsg`, `ServerMsg`, `LastMessage`).
- Four patterns: realtime send (WS→MQTT), poll (HTTP→cached MQTT), realtime receive (MQTT→WS push),
  and ping request/response with UUID correlation.

**Integration gap (future work):** the demo's topics
(`egui-axum-mqtt-demo/{send,poll,live,ping/request,ping/response}`) don't yet align with this device's
contract (`host-esp32c6/mbox` + the `Command` enum). The README roadmap calls for defining unified MQTT
message formats across `cmd` topics so the backend can drive this device directly.

## Key Files

- `host-esp32c6/src/bin/main.rs` — Embassy entrypoint, hardware init, spawns all async tasks
- `host-esp32c6/src/lib.rs` — shared globals (signals/atomics/`MODE`/`DIRECT_CMD`), `log!`, `Mode`/`Command`/`DirectCommand` types
- `host-esp32c6/src/wasm.rs` — Wasmi engine setup + frame loop (embeds `guest.wasm`)
- `host-esp32c6/src/led.rs` — LED strip driver (RMT + serpentine mapping + gamma/brightness)
- `host-esp32c6/src/mqtt.rs` — MQTT v5 client, command parsing + dispatch
- `host-esp32c6/src/direct.rs` — Direct mode (apply `DirectCommand`s to the pixel buffer)
- `host-esp32c6/src/net.rs` — WiFi connection + network stack tasks
- `guest/src/lib.rs` — all guest animations and the `update()` dispatch
- `guest/build.rs` — Aseprite JSON → Rust frame-table code generation
- `host-common/src/lib.rs` — serpentine index mapping (with unit tests)
- `common/src/lib.rs` — panel dimension constants + pixel helpers shared by guest and host
