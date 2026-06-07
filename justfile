default:
    just --list

# Build everything
build: build-guest build-host

# Build the guest WASM application
build-guest:
    just -f guest/justfile build

# Build the host Wasmi application
build-host:
    just -f host-esp32c6/justfile build

# Build, upload and run the host Wasmi application on an ESP32-C6
run: build
    just -f host-esp32c6/justfile run

clean:
    just -f guest/justfile clean
    just -f host-esp32c6/justfile clean

ci: build-guest
    just -f guest/justfile ci
    just -f host-esp32c6/justfile ci
    cargo clippy -p backend -- -D warnings
    cargo clippy -p frontend --target wasm32-unknown-unknown -- -D warnings
    cargo fmt --check

fmt:
	cargo fmt

# Tests are only for non-embedded crates
test:
    cargo test -p host-common

# --- Web stack (browser + backend tiers) ---
# Run each in its own terminal; bring up the broker (`just mosquitto`) first.

# Run the axum backend (0.0.0.0:3000, connects to localhost:1883)
run-backend:
    cargo run --package backend

# Serve the egui/WASM frontend with hot-reload (trunk on :8080, proxies /api -> :3000)
run-frontend:
    just -f frontend/justfile run

# Build the web stack (backend binary + frontend WASM into backend/dist)
build-web:
    cargo build -p backend
    just -f frontend/justfile build

# Backend integration tests — requires a running broker (`just mosquitto`)
test-backend:
    cargo test --package backend --test integration

mosquitto:
    docker network remove mqtt || true
    docker network create mqtt
    docker run \
        --rm \
        -it \
        --name mqtt-broker \
        --network mqtt \
        -p 1883:1883 \
        -v "$PWD/mosquitto/config:/mosquitto/config" \
        -v /mosquitto/data \
        -v /mosquitto/log \
        eclipse-mosquitto

mosquitto-monitor:
    docker run -it --network mqtt eclipse-mosquitto mosquitto_sub -d -h mqtt-broker -p 1883 -t '#' -v
