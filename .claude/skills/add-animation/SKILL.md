---
name: add-animation
description: Add a new LED animation or image asset to the guest WASM program. Use when adding/editing animations, importing a PNG/GIF/Aseprite asset, or wiring a new pattern into the guest update() dispatch in the esp32-wasmi-led project.
---

# Add a Guest Animation (esp32-wasmi-led)

Animations live in the `guest` crate (compiled to WASM, runs on the device under Wasmi). The guest
exports `update(ticks, frame, host_buffer_offset) -> u32`, returning a byte offset into WASM linear
memory pointing at a **768-byte** (16×16×3 RGB) buffer. Timing is **256 ticks/second**.

## Dispatch model (`guest/src/lib.rs::update()`)

First 512 ticks = boot test (`white` → `corners`). After that it cycles `ticks % 4096`:

| Tick range  | Function         |
|-------------|------------------|
| 0–1024      | `rainbow_cycle`  |
| 1024–2048   | `proc0001`       |
| 2048–3072   | `anim0001`       |
| 3072–4096   | `anim0002`       |

Each fn returns either `host_buffer_offset` (write into the host-provided buffer via
`common::set_color`/`set_all`) **or** a pointer to a guest static buffer cast to `u32` (e.g. for
prebaked image frames).

## Steps

### 1. Add the asset
Put the source file in `guest/assets/` (e.g. `my-anim.gif`, `my-image.png`, or an Aseprite export).

### 2. Convert to raw RGB (`guest/justfile`, `build-assets` recipe)
Add a `convert` (ImageMagick) line producing raw RGB into `guest/target/`:

```sh
# static 16x16 image -> one frame
convert assets/my-image.png -depth 8 rgb:target/my-image.raw
# animated GIF -> one .raw per frame (my-anim_000.raw, my-anim_001.raw, ...)
convert assets/my-anim.gif -coalesce -depth 8 rgb:target/my-anim_%03d.raw
# vertical sprite-sheet PNG -> single concatenated .raw (16x16 frames stacked)
convert assets/my-sheet.png -depth 8 rgb:target/my-sheet.raw
```

### 3. (Aseprite sprite sheets only) frame timing via build.rs
Export the Aseprite JSON next to the asset. `guest/build.rs` parses it into a `FRAME_OFFSETS` table
(durations converted ms → ticks, 256 ticks/sec) emitted to `OUT_DIR`. The current `build.rs` is
hardwired to `assets/anim-0002.json` — generalize it (or copy its pattern) for a new sheet, and add a
`cargo:rerun-if-changed` for the new JSON.

### 4. Embed + implement (`guest/src/lib.rs`)
```rust
static MY_FRAMES: [&[u8; 768]; N] = [
    include_bytes!("../target/my-anim_000.raw"),
    // ...
];

#[unsafe(no_mangle)]
pub extern "C" fn my_anim(ticks: u64, _frame: u64, _host_buffer_offset: u32) -> u32 {
    let frame = ticks * FPS / TICKS_PER_SECOND;          // FPS/TICKS_PER_SECOND consts already defined
    let idx = (frame % MY_FRAMES.len() as u64) as usize;
    MY_FRAMES[idx].as_ptr() as u32                        // return static-buffer pointer
}
```
For procedural patterns, write into the host buffer instead:
```rust
let ptr = host_buffer_offset as *mut u8;
unsafe { set_color(ptr, (x, y), (r, g, b)); }
return host_buffer_offset;
```
Coordinates use a **top-left origin** `(0,0)`; the host handles serpentine remapping + gamma + brightness.

### 5. Wire into `update()`
Add/extend a match arm in the `ticks % 4096` dispatch (adjust ranges as needed):
```rust
3072..4096 => anim0002(ticks - 3072, frame, host_buffer_offset),
4096..     => my_anim(ticks - 4096, frame, host_buffer_offset),  // remember to widen the modulo
```

### 6. Rebuild
`just build` (assets → guest → host). The guest must rebuild before the host, which embeds `guest.wasm`.

## Inspecting assets (optional)
```sh
just -f guest/justfile gallery   # montage of all converted frames (needs ImageMagick `montage`)
just -f guest/justfile opt       # wasm-opt size pass (needs wasm-opt)
just -f guest/justfile print     # wasm-tools text dump
```
