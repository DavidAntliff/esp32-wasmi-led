# ESP32 Wasmi LED

This is a project using the [Wasmi WebAssembly Interpreter](https://github.com/wasmi-labs/wasmi)
with an ESP32-C6 microcontroller (`no_std` + `alloc`) to control an LED matrix.

`host-esp32c6` is a **riscv32imac-unknown-none-elf** targeted application that provides
some host functions and executes a built-in WASM program.

`guest` is a WebAssembly program that, when run on the host, makes use of the host
function bindings to access host features and rotate the LED matrix through various patterns.

The host app assumes a grid of 16x16 WS21812 LEDs connected to GPIO10 in a sequential serpentine
arrangement. A Wokwi configuration is provided to simulate this, if such hardware is not available.

## Notes

Use Cases:

* Load a static PNG via the web app, have it displayed.
* Load an animated GIF, have it displayed with correct frame timing.
* Load a PNG animation strip and timing file, have it displayed with correct timing.
* Allow manual "painting" of the grid, have it displayed in realtime.
* Provide custom controls in the web app UI that can be manipulated to affect the active display.
* Provide a code-console to allow a WASM guest to be written and uploaded to the device.
* Provide a way to load any WASM guest module via the backend.
* Persistence - store frames, patterns, guest programs, etc. for easy retrieval and selection in the web app.
* Security - yeah, sure, that sounds like a good idea.
* Realtime audio/event data for syncing display to sound/music. Needs some thought.
* Games? Multiplayer pong?
* Cellular Automata
* Power estimation for brightness control

The Plan:

* Refactor host-esp32c6 so that a native "emulator" can be built, that displays a matrix and responds to MQTT messages.
* Define MQTT message formats across several `cmd` topics, for controlling system, led matrix, wasm host, wasm guest,
  etc.
* Bring in front & backend components from egui-axum-mqtt-demo, to build the web-app.

Ideas for Guest Apps

* Painting app,
* Clock - analogue, digital, binary,
* Interactive Pong, Auto-Pong
* Interactive Snake, Auto-Snake
* Cellular Automata (Life, Termites, etc) - could use a starting image as input

Handy to let the image buffer be persistent, so that, for example, can cross-fade from one guest to another, or a game
or cellular-automata can use a previous image as a starting point. This creates a lot of emergent activities.

## Use of AI

The author acknowledges the use of the help of AI tools such as Microsoft Copilot and Anthropic Claude Code.

Use of AI includes architectural discussions, API guidance, local auto-completion in parts, and boilerplate code
generation, as well as some front-end code. AI has also been used to generate some of the documentation in this
repository.

This repository was not "vibe coded", and the author is aware and has a working knowledge of all code in this
repository.
