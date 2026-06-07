# Roadmap: Packaging the web stack (mosquitto + backend + frontend)

> **Status: DEFERRED — design captured, not implemented.**
> For now we keep running the three web-tier pieces by hand in three terminals:
>
> ```sh
> just mosquitto       # MQTT broker on :1883  (eclipse-mosquitto in Docker)
> just run-backend     # axum on :3000, connects localhost:1883
> just run-frontend    # trunk serve on :8080, hot-reload, proxies /api -> :3000
> ```
>
> This document records the analysis and the agreed design so the work can be picked up later
> without re-deriving it. It implements the AGENTS.md roadmap item: "package all three tiers with
> docker-compose."

## Why / the question

The web stack is started as three manual terminals. The question was whether a **docker-compose
"stack"** with **locally-compiled binaries bind-mounted in** makes sense, and whether there's a better
way than three terminal windows.

## Key findings (the shape of the problem)

- **It's really 2 services + a build step, not 3 symmetric services.**
  - `mosquitto` is *already* a container (`just mosquitto` → `eclipse-mosquitto`, config bind-mounted
    from `mosquitto/config/`). Trivial to move into compose.
  - `backend` is the one native long-running service.
  - `frontend` is **not** a runtime service in production — `trunk build` emits static WASM to
    `backend/dist/`, which the backend serves via `ServeDir::new("dist")` (`backend/src/lib.rs:168`).
    `trunk serve` (:8080) is only a *dev* hot-reload convenience that proxies `/api` → :3000
    (`frontend/Trunk.toml`).
- **Backend connection is hardcoded:** `create_mqtt("egui-axum-mqtt-backend", "localhost", 1883, …)` and
  `bind("0.0.0.0:3000")` in `backend/src/main.rs:10,16`. But `create_mqtt()`
  (`backend/src/lib.rs:61`) already *takes* host/port params — so only `main.rs` needs to read them from
  env. Inside a compose network the broker is reachable as service name `mqtt`, not `localhost`.
- **Device LAN constraint:** the ESP32 dials `192.168.1.201:1883`, and `mqtt_task` **exits at boot** if
  the broker is absent (no retry until reboot). So compose must **publish** 1883 to the host and run on
  the host the device targets. Mosquitto already listens `0.0.0.0`
  (`mosquitto/config/mosquitto.conf`).
- **musl/TLS snag:** `Cargo.lock` pulls **both** `aws-lc-rs` and `ring` (via rumqttc `use-rustls`).
  `aws-lc-sys` needs cmake/clang and is the painful dependency for a static-musl build. The broker is
  **plaintext** (no TLS), so the whole TLS stack is dead weight here.

## Decisions made (from Q&A)

| Question | Decision |
|----------|----------|
| Which setup? | **Both** — a docker-compose packaged stack **and** a lightweight one-command dev supervisor. |
| Frontend? | **Hot-reload `trunk serve`** as a service (in both paths). |
| Backend binary for the container? | **Static musl binary** (`x86_64-unknown-linux-musl`), bind-mounted in. |

### Honest framing
- docker-compose makes sense and matches the roadmap, but its real value is **one-command lifecycle +
  ordering + a packaged artifact** — *not* the daily loop.
- For iteration, a **process supervisor** (e.g. `process-compose`, or the lighter `mprocs`) is better:
  one terminal, all logs in panes, one Ctrl-C stops everything, hot reload preserved, **no** binary
  packaging. Hence: build both.

---

## Implementation plan (when resumed)

### Step 1 — Make the backend container-configurable (small code change)
`backend/src/main.rs` — read env with sensible defaults so the same binary works locally and in compose:
- `MQTT_HOST` (default `localhost`), `MQTT_PORT` (default `1883`) → into existing `create_mqtt(...)`.
- `BIND_ADDR` (default `0.0.0.0:3000`) for the `TcpListener::bind`.
- (optional) `MQTT_CLIENT_ID` (default `egui-axum-mqtt-backend`).

~8 lines via `std::env::var(...).unwrap_or_else(...)`. No change to `lib.rs` (already parameterized).
In compose, backend gets `MQTT_HOST=mqtt`.

### Step 2 — `just dev` supervisor (daily inner loop)
Add **`process-compose.yaml`** with three local processes:
- **mosquitto** — detached form of the existing recipe (`docker run --rm --name mqtt-broker -p 1883:1883
  -v "$PWD/mosquitto/config:/mosquitto/config" eclipse-mosquitto`), **no `-it`** so it runs unattended.
- **backend** — `cargo run -p backend` with `MQTT_HOST=localhost`.
- **frontend** — `trunk serve --port 8080` (cwd `frontend/`).

Gate backend/frontend on a broker-ready probe (TCP :1883). Add `just dev` → `process-compose up`.

### Step 3 — `compose.yaml` packaged stack
Three services on a user-defined `mqtt` network:
- **mqtt** — `image: eclipse-mosquitto`, bind-mount `./mosquitto/config`, `ports: 1883:1883`, data/log
  volumes, a healthcheck. Replaces the manual `docker run`.
- **backend** — minimal base (`alpine`, for a shell-based healthcheck), **bind-mount the locally built
  musl binary** (`./target/x86_64-unknown-linux-musl/release/backend:/app/backend:ro`),
  `command: /app/backend`, env `MQTT_HOST=mqtt MQTT_PORT=1883`, `ports: 3000:3000`,
  `depends_on: mqtt (service_healthy)`.
- **frontend** — hot-reload dev service: a rust+trunk image, bind-mount repo source + a named volume for
  the cargo/target cache, `command: trunk serve --address 0.0.0.0 --port 8080` proxying to
  `http://backend:3000`, `ports: 8080:8080`.
  - **Tradeoff to flag:** this service *compiles in-container* — the one place "local toolchain only"
    doesn't hold, because hot-reload requires a live compiler where trunk runs.
  - **Alternative:** drop this service and have the backend serve a locally-built static `dist/`
    (`trunk build` → bind-mount `backend/dist`). Lighter, but loses hot reload.

Add `just stack-up` / `just stack-down`. Run compose on the host the device dials (`192.168.1.201`).

### Step 4 — Static musl backend build
- Add `x86_64-unknown-linux-musl` to `targets` in `rust-toolchain.toml` (host may need `musl-tools` /
  `musl-gcc`).
- `just build-backend-musl` → `cargo build -p backend --release --target x86_64-unknown-linux-musl`.
- **Resolve the TLS dep so static musl links cleanly:**
  - *Recommended:* drop `use-rustls` from rumqttc in `backend/Cargo.toml` (broker is plaintext 1883,
    TLS is unused) — removes `rustls`/`ring`/`aws-lc-rs` entirely.
  - *Fallback (if TLS must stay):* pin rustls to the `ring` provider so `aws-lc-sys` (cmake/clang) is
    dropped.

### Step 5 — Trunk proxy for the container
`frontend/Trunk.toml` proxies to `http://localhost:3000`; the frontend compose service must proxy to the
`backend` service name. Provide a container override — CLI flags on `trunk serve`
(`--proxy-backend http://backend:3000/api/`, plus the ws entry) or a `frontend/Trunk.container.toml`
passed with `trunk serve --config`. The host `just dev` path keeps the existing `Trunk.toml`.

### Step 6 — Recipes & docs
- New `just` recipes: `dev`, `stack-up`, `stack-down`, `build-backend-musl`.
- Update `AGENTS.md` (mark the roadmap docker-compose item done; document the two run paths),
  `CLAUDE.md` working notes, and `README.md` run section.
- **Secrets:** the backend needs **no** WiFi vars (those are device-compile-time only) — keep `.envrc`
  and `WIFI_*` out of compose/process-compose entirely.

---

## Files to touch (when resumed)

- `backend/src/main.rs` — env-configurable MQTT host/port + bind addr.
- `backend/Cargo.toml` — drop `use-rustls` (or pin rustls→ring) for the musl build. *(optional, recommended)*
- `compose.yaml` — **new**, 3 services.
- `process-compose.yaml` — **new**, 3 dev processes.
- `frontend/Trunk.container.toml` — **new** (or CLI proxy flags) for the containerized trunk serve.
- `rust-toolchain.toml` — add `x86_64-unknown-linux-musl`.
- `justfile` — `dev`, `stack-up`, `stack-down`, `build-backend-musl`.
- `AGENTS.md`, `CLAUDE.md`, `README.md` — document both run paths.

## Verification (when resumed)

- **musl build:** `just build-backend-musl` succeeds; `file target/x86_64-unknown-linux-musl/release/backend`
  reports a statically linked ELF.
- **Dev supervisor:** `just dev` → process-compose TUI shows mosquitto ready, backend
  `Listening on http://localhost:3000`, trunk on :8080; open http://localhost:8080; edit a frontend file
  and confirm hot reload; "Ping Device" round-trips (requires the ESP32 online on the same broker).
- **Compose stack:** `just stack-up` → `docker compose ps` all healthy; `curl localhost:3000/api/last-message`
  responds; http://localhost:8080 loads; `mosquitto_sub` on host `:1883` sees traffic; ping round-trips
  with the device. `just stack-down` tears it all down.
- **Still green:** `just ci` passes; `just test-backend` (broker up) passes.

## Open tradeoffs to settle at implementation time

- Hot-reload frontend in a container compiles in-container (Step 3) — accept it, or switch to the
  static-dist alternative.
- musl + `aws-lc-rs`: prefer dropping TLS (plaintext broker) vs. pinning to `ring`.
- The device's LAN dependency on the broker is unchanged — compose just has to publish 1883 on the host
  the device targets.
