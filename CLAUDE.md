# CLAUDE.md

**Read [AGENTS.md](./AGENTS.md) first.** It is the source of truth for architecture, hardware, the MQTT
control protocol, the build/run sequence, and conventions. This file covers only Claude-specific
working notes that aren't obvious from the code.

Project-specific capabilities live in `.claude/skills/`: **build-and-flash**, **mqtt-control**, and
**add-animation**. Prefer them for those workflows.

## Working notes

- **Build order is a hard constraint.** The host embeds the compiled guest via
  `include_bytes!(".../release/guest.wasm")`, and the guest itself `include_bytes!`s asset `.raw` files
  produced by `build-assets`. Always go through `just build` (assets → guest → host) rather than ad-hoc
  `cargo build`, or you'll build the host against a stale/missing guest.
- **Targets when running cargo by hand.** guest → `--target=wasm32-unknown-unknown`; host-esp32c6 →
  `--target=riscv32imac-unknown-none-elf`; `common`/`host-common`/`dummy` build natively.
- **Don't trust a bare `cargo check`.** `default-members` is only `dummy` + `host-common`, so a
  root-level `cargo check`/`build` skips the cross-compiled crates **and the web crates** and can look
  green while `guest`/`host-esp32c6`/`frontend` is broken. Use `-p <crate>` with the right `--target`, or
  `just build` / `just ci`.
- **Web stack (`backend`/`frontend`/`web-common`).** Imported from `egui-axum-mqtt-demo`; **not** in
  `default-members`. `backend`/`web-common` build natively (`-p`), but `frontend` is **wasm-only** — build
  it with **trunk** (`just run-frontend` / `just build-web` → `backend/dist/`; `cargo install trunk
  --locked`), not bare `cargo build`. `just test-backend` runs the backend integration tests but **needs a
  running broker** (`just mosquitto`). The backend dials `localhost:1883`, so it must run on the same host
  as the broker the device uses (`192.168.1.201`) for the ping to round-trip; the MQTT prefix
  `esp32-wasmi-led` (`DEFAULT_PREFIX` in `backend/src/lib.rs`) must match the device's
  `PING_REQ_TOPIC`/`PING_RESP_TOPIC` in `mqtt.rs`.
- **Tests.** Only `cargo test -p host-common` (and `just test-backend`, needs a broker) work. The guest
  and host-esp32c6 are `no_std` embedded/WASM targets with no test harness — don't try to `cargo test` them.
- **Linting / "is it green?".** There is no GitHub CI (`.github/` is empty). Run `just ci` (clippy
  `-D warnings` on both targets + `fmt --check`) before claiming the build is clean.
- **Logging.** Use the `log!` macro (`host-esp32c6/src/lib.rs`); it dual-emits to defmt (RTT) and
  `esp_println` (UART). Don't add bare `println!` or `defmt::info!`. Use `defmt::Debug2Format` /
  `Display2Format` for types that don't implement `defmt::Format`.
- **Secrets & LAN values.** `WIFI_SSID`/`WIFI_PASSWORD` are compile-time env vars — never hardcode or
  commit them. The hardcoded IPs (`BROKER_IP` in `mqtt.rs`, static IP in `main.rs`) are the user's
  network; flag and confirm before changing them.
- **Flashing is via probe-rs, not espflash** (over the ESP32-C6's USB-JTAG). Don't suggest `espflash`.
  The cable must be on the board's **right-most port labelled "USB"** (native USB-JTAG); the other port
  won't flash or stream serial.
- **Running it.** `just run` flashes and then **stays attached, streaming defmt logs indefinitely** — it
  never exits on success. Launch it with `run_in_background` and watch the output file (or a Monitor) for
  the boot sequence (`🦀 …` → `Got IP` → `WASMI entering main loop`) and for failures — notably USB
  `device disconnected` (reseat the cable) and MQTT `TCP connect failed` (no broker). Confirm MQTT
  commands landed by grepping the run log for `Parsed command:` / `SetPixel:` / `Failed to parse`.
- **Broker.** You may start it yourself with `just mosquitto`, but its recipe uses `docker run -it`
  (needs a TTY) — for an unattended background start, run it detached / without `-it`. The device only
  dials `192.168.1.201`, so the broker must be on that host (or change `BROKER_IP`), and `mqtt_task`
  exits if it's absent at boot (no retry until reboot).
