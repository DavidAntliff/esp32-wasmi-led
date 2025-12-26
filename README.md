# ESP32 WASMI Demo

Simple project demonstrating the use of the [Wasmi WebAssembly Interpreter](https://github.com/wasmi-labs/wasmi)
with an ESP32-C6 microcontroller.

`host-esp32c6` is a simple **riscv32imac-unknown-none-elf** targeted application that provides
some host functions and executes a built-in WASM program.

`guest-fibonacci` is a simple WebAssembly program that, when run on the host, makes use of the host
function bindings to access host features.


