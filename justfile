default:
    just --list

# Build everything
build: build-guest build-host run

# Build the guest WASM application
build-guest:
    just -f guest-fibonacci/justfile build

# Build the host Wasmi application
build-host:
    just -f host-esp32c6/justfile build

# Build, upload and run the host Wasmi application on an ESP32-C6
run:
    just -f host-esp32c6/justfile run

clean:
    just -f guest-fibonacci/justfile clean
    just -f host-esp32c6/justfile clean
