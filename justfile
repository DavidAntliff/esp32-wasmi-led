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

ci:
    just -f guest/justfile ci
    just -f host-esp32c6/justfile ci
