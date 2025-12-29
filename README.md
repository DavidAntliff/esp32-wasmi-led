# ESP32 Wasmi LED

This is a project using the [Wasmi WebAssembly Interpreter](https://github.com/wasmi-labs/wasmi)
with an ESP32-C6 microcontroller (`no_std` + `alloc`) to control an LED matrix.

`host-esp32c6` is a **riscv32imac-unknown-none-elf** targeted application that provides
some host functions and executes a built-in WASM program.

`guest-fill` is a simple WebAssembly program that, when run on the host, makes use of the host
function bindings to access host features and set the entire matrix to a single colour.

The host app assumes a grid of 16x16 WS21812 LEDs connected to GPIO10 in a sequential serpentine
arrangement. A Wokwi configuration is provided to simulate this, if such hardware is not available.

