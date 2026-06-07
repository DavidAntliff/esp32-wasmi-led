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

Eight workspace crates (`Cargo.toml`, resolver `"3"`, edition 2024). The five **device-tier** crates are
below; the three **web-stack** crates (`web-common`, `backend`, `frontend`) are covered under
[Web stack](#web-stack-backend--frontend). Device-tier dependencies:
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
- **Ping liveness**: also subscribes to `esp32-wasmi-led/ping/request`; on each ping it echoes the
  request's `correlation_id` and publishes `{"correlation_id":"<id>","message":"pong from host-esp32c6"}`
  to `esp32-wasmi-led/ping/response`. This backs the backend's "Ping Device" round-trip. The topic prefix
  must match the backend's `DEFAULT_PREFIX`; see `PING_REQ_TOPIC`/`PING_RESP_TOPIC` in `mqtt.rs` and
  [Web stack](#web-stack-backend--frontend).
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

## Web stack (backend + frontend)

Imported from the `egui-axum-mqtt-demo` template, these three crates provide the **browser → backend**
tiers. They are **not** in `default-members` and are built per-crate via `just`/`trunk` (see the root
`justfile`), never by a bare `cargo build`.

| Crate        | Target      | Role                                                                                    |
|--------------|-------------|-----------------------------------------------------------------------------------------|
| `web-common` | native/WASM | Shared `serde` WS/HTTP message types (`ClientMsg`, `ServerMsg`, `LastMessage`)           |
| `backend`    | native      | `axum` 0.8 server; bridges WebSocket/HTTP ↔ MQTT (`rumqttc` 0.25); serves the frontend   |
| `frontend`   | `wasm32`    | `eframe`/`egui` 0.33 app, built by `trunk`; talks to `backend` over WebSocket + HTTP GET |

> Naming: the template's `common` crate was imported as **`web-common`** to avoid clashing with this
> repo's existing device-tier `common`.

- **Ports**: MQTT broker `1883`, backend `0.0.0.0:3000`, trunk dev server `8080`.
- **MQTT topic prefix**: `esp32-wasmi-led` (`DEFAULT_PREFIX` in `backend/src/lib.rs`) — topics
  `esp32-wasmi-led/{send,poll,live,ping/request,ping/response}`. Only `ping/request`→`ping/response` is
  wired to the device today (see [MQTT Control Protocol](#mqtt-control-protocol)); `send`/`poll`/`live`
  are demo plumbing not yet mapped to the device's `Command` protocol.
- **Broker host**: the backend connects to `localhost:1883`, so it must run on the **same host** as the
  broker the device dials (`192.168.1.201`) for the ping to round-trip end-to-end.
- **Frontend serving**: `trunk build` emits the WASM bundle to `backend/dist/` (git-ignored) and the
  backend serves it via `ServeDir`; in dev, `trunk serve` (8080) proxies `/api` → backend (3000) instead.
- **The behaviour is the imported demo's, unchanged** (realtime send, poll, live receive, ping) — only
  ping does anything real against the device so far.

Run it (each in its own terminal; **broker first**):

```sh
just mosquitto       # MQTT broker on :1883
just run-backend     # axum on :3000  (logs "Listening on http://localhost:3000")
just run-frontend    # trunk serve on :8080 (hot-reload), proxies /api -> :3000
# open http://localhost:8080  →  "Ping Device" round-trips to the ESP32-C6
```

`just build-web` builds both tiers (frontend WASM into `backend/dist/`); `just test-backend` runs the
backend integration tests (needs a running broker). **Trunk** is an extra prerequisite:
`cargo install trunk --locked` (uses the `wasm32-unknown-unknown` target already in `rust-toolchain.toml`).

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

This repo now contains all three tiers of the system:

```
browser (egui/WASM) ──WS/HTTP──► axum backend ──MQTT──► ESP32-C6 (this repo)
```

The front+back tiers were brought in from the sibling template **`../egui-axum-mqtt-demo`** and live here
as the `frontend`, `backend`, and `web-common` crates (see [Web stack](#web-stack-backend--frontend)).
They keep the template's behaviour — four patterns: realtime send (WS→MQTT), poll (HTTP→cached MQTT),
realtime receive (MQTT→WS push), and ping request/response with UUID correlation.

**What's wired so far:** the ping round-trip reaches the device — the browser's "Ping Device" publishes to
`esp32-wasmi-led/ping/request`, the firmware replies on `esp32-wasmi-led/ping/response`, and the backend
relays the pong back to the browser.

**Integration gap (next):** the demo's other topics (`esp32-wasmi-led/{send,poll,live}`) still don't map
to this device's control contract (`host-esp32c6/mbox` + the `Command`/`DirectCommand` enums). The plan is
to define unified MQTT message formats so the backend can drive LED modes/pixels directly, then package
all three tiers with docker-compose.

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
- `web-common/src/lib.rs` — shared WS/HTTP message types (`ClientMsg`/`ServerMsg`/`LastMessage`)
- `backend/src/lib.rs` — axum router + MQTT↔WS/HTTP bridge (`DEFAULT_PREFIX`, `Topics`, `PingPayload`)
- `backend/src/main.rs` — backend entrypoint (binds `0.0.0.0:3000`, connects `localhost:1883`)
- `frontend/src/app.rs` — egui UI + WebSocket/HTTP client tasks
- `frontend/Trunk.toml` — trunk config (dist → `backend/dist`, dev proxy → `:3000`)
