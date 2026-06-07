---
name: build-and-flash
description: Build, lint, test, and flash the ESP32-C6 firmware. Use whenever building the project, running it on hardware, flashing the device, or debugging build-order/toolchain/probe-rs issues in the esp32-wasmi-led repo.
---

# Build & Flash (esp32-wasmi-led)

This is a Cargo workspace with cross-compiled crates. **Always build through `just`** so the guest WASM
and its assets are built before the host firmware that embeds them.

## One-time setup

```sh
rustup component add rust-src                                  # required by build-std
rustup target add riscv32imac-unknown-none-elf wasm32-unknown-unknown
cargo install just probe-rs-tools                              # task runner + flasher
# ImageMagick (`convert`) for guest assets; Docker for the MQTT broker
```

`rust-toolchain.toml` normally auto-installs the component + targets; the commands above are the manual
fallback.

## Required before building the host

WiFi credentials are read at **compile time** via `env!()` — the host build fails without them. They
live in a **git-ignored `.envrc`** (loaded by direnv, or `source .envrc`) that exports:

```sh
export WIFI_SSID="…"        # in .envrc — NEVER commit real values
export WIFI_PASSWORD="…"    # in .envrc — NEVER commit real values
```

Reference the variable names (`$WIFI_SSID` / `$WIFI_PASSWORD`) in commands; never write the literal
values into source, docs, command history, or logs.

If targeting a different network, also edit the hardcoded IPs first:
- `host-esp32c6/src/mqtt.rs` → `BROKER_IP` (MQTT broker, default `192.168.1.201`)
- `host-esp32c6/src/bin/main.rs` → device static IP / gateway / DNS (default `192.168.1.242/24`)

## Commands

```sh
just build   # guest assets -> guest WASM -> host firmware (correct order — don't skip)
just ci      # clippy -D warnings on BOTH targets + cargo fmt --check
just test    # unit tests (host-common only)
just run     # build + flash + run on a USB-connected ESP32-C6 via probe-rs
just clean   # clean guest and host
```

## Running & monitoring

`just run` flashes and then **stays attached, streaming defmt logs indefinitely** — it does **not** exit
on success. Run it in the background and watch the output rather than blocking on it:

- **Expect:** `🦀 WASM LED Matrix Host` → `Wifi connected!` → `Got IP: …` → `WASMI entering main loop`
  (and `Connected to MQTT broker` if a broker is up).
- **Watch for:** `USB Communication Error: device disconnected` (cable/port dropped — reseat and re-run),
  `TCP connect failed` (no MQTT broker reachable), `panic` / `out of bounds`.

## Why build order matters

- The host embeds the guest at compile time:
  `include_bytes!("../../target/wasm32-unknown-unknown/release/guest.wasm")` (`host-esp32c6/src/wasm.rs`).
- The guest embeds asset `.raw` files produced by `guest/justfile`'s `build-assets`.

So: **assets → guest → host**. `just build` enforces this. A bare `cargo build` at the workspace root
only covers `default-members` (`dummy` + `host-common`) and will silently skip the cross-compiled crates.

## Flashing details

- Runner (`host-esp32c6/.cargo/config.toml`):
  `probe-rs run --chip=esp32c6 --probe 303a:1001 --preverify --always-print-stacktrace --catch-hardfault`.
- `303a:1001` is the ESP32-C6's **built-in USB-JTAG** — connect the board over USB; no external probe.
- **Use the board's right-most port, labelled "USB"** (native USB-JTAG). The other port (UART bridge)
  will not flash or stream serial.
- **Not espflash.** Don't suggest `espflash` for flashing.
- Logs come out over RTT (defmt) and UART (`esp_println`) via the `log!` macro; `DEFMT_LOG=info` by
  default. defmt *locations* may print as `<invalid location: defmt frame-index: N>` under probe-rs —
  cosmetic; the messages decode fine.

## No hardware?

A Wokwi simulation is configured at `host-esp32c6/wokwi.toml` + `diagram.json`.
