#![no_std]
#![no_main]

use core::cell::UnsafeCell;

// Panic handler required for no_std
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}

// External function provided by the host
// unsafe extern "C" {
//     // fn output(num: u64);
//     //
//     // fn set_pixel(x: u32, y: u32, r: u32, g: u32, b: u32);
//     //
//     // fn fill(r: u32, g: u32, b: u32);
//     //
//     // fn update();
// }

// Don't call the entry 'main' as it will get wrapped with C-style (argc, argv) parameters

const TICKS_PER_SECOND: u64 = 256; // TODO: move to SDK

const FPS: u64 = 60; // animation target speed

const PIXEL_HEIGHT: usize = 16;
const PIXEL_WIDTH: usize = 16;
const PIXEL_CHANNELS: usize = 3; // RGB
const PIXEL_BUFFER_SIZE: usize = PIXEL_HEIGHT * PIXEL_WIDTH * PIXEL_CHANNELS;

// SAFETY: WASM is single-threaded, so this is safe
struct SyncWrapper<T> {
    inner: UnsafeCell<T>,
}

unsafe impl<T> Sync for SyncWrapper<T> {}

static PIXEL_BUFFER: SyncWrapper<[u8; PIXEL_BUFFER_SIZE]> = SyncWrapper {
    inner: UnsafeCell::new([0u8; PIXEL_BUFFER_SIZE]),
};

//static STATIC_0001_IMAGE_DATA: &[u8] = include_bytes!("../assets/static-0001.raw");

static ANIM_0001_IMAGE_DATA: [&[u8; 768]; 6] = [
    include_bytes!("../target/anim-0001_000.raw"),
    include_bytes!("../target/anim-0001_001.raw"),
    include_bytes!("../target/anim-0001_002.raw"),
    include_bytes!("../target/anim-0001_003.raw"),
    include_bytes!("../target/anim-0001_004.raw"),
    include_bytes!("../target/anim-0001_005.raw"),
];

#[inline(always)]
fn offset(x: usize, y: usize) -> usize {
    (y * PIXEL_WIDTH + x) * PIXEL_CHANNELS
}

#[inline(always)]
fn set_color(buffer: *mut u8, (x, y): (usize, usize), (r, g, b): (u8, u8, u8)) {
    if x >= PIXEL_WIDTH || y >= PIXEL_HEIGHT {
        return; // Out of bounds
    }
    let offset = offset(x, y);
    unsafe {
        buffer.add(offset).write(r);
        buffer.add(offset + 1).write(g);
        buffer.add(offset + 2).write(b);
    }
}

fn clear(buffer: *mut u8, color: (u8, u8, u8)) {
    // TODO: optimize
    for y in 0..PIXEL_HEIGHT {
        for x in 0..PIXEL_WIDTH {
            set_color(buffer, (x, y), color);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn buffer_ptr() -> *mut u8 {
    PIXEL_BUFFER.inner.get() as *mut u8
}

#[unsafe(no_mangle)]
pub extern "C" fn mem_write(offset: u32, value: u32) {
    let ptr = buffer_ptr();
    if offset as usize >= PIXEL_BUFFER_SIZE {
        panic!("Offset out of bounds");
    }
    unsafe {
        ptr.add(offset as usize).write(value as u8);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mem_read(offset: u32) -> u32 {
    let ptr = buffer_ptr();
    if offset as usize >= PIXEL_BUFFER_SIZE {
        panic!("Offset out of bounds");
    }
    unsafe { ptr.add(offset as usize).read() as u32 }
}

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    let ptr = buffer_ptr();
    unsafe {
        core::ptr::write_bytes(ptr, 0, PIXEL_BUFFER_SIZE);
    }
}

/// Returns the offset to the pixel buffer to be displayed
#[unsafe(no_mangle)]
pub extern "C" fn update(ticks: u64, frame: u64, host_buffer_offset: u32) -> u32 {
    const BOOT_TICKS: u64 = 512;

    // 256 ticks per second
    if ticks < BOOT_TICKS {
        // Initial white + corners test pattern for the first 2 seconds
        match ticks % BOOT_TICKS {
            0..256 => white(ticks, frame, host_buffer_offset),
            256.. => corners(ticks, frame, host_buffer_offset),
        }
    } else {
        let ticks = ticks - BOOT_TICKS;
        match ticks % 4096 {
            0..1024 => rainbow_cycle(ticks, frame, host_buffer_offset),
            1024..2048 => proc0001(ticks - 1024, frame, host_buffer_offset),
            2048..3072 => anim0001(ticks - 2048, frame, host_buffer_offset),
            3072.. => anim0002(ticks - 3072, frame, host_buffer_offset),
        }
    }
}

fn corners(_ticks: u64, _frame: u64, host_buffer_offset: u32) -> u32 {
    // Use the host-provided buffer
    let ptr = host_buffer_offset as *mut u8;

    clear(ptr, (0, 0, 0));
    set_color(ptr, (0, 0), (255, 0, 0)); // top-left is red
    set_color(ptr, (PIXEL_WIDTH - 1, 0), (0, 255, 0)); // top-right is green
    set_color(ptr, (0, PIXEL_HEIGHT - 1), (0, 0, 255)); // bottom-left is blue
    set_color(ptr, (PIXEL_WIDTH - 1, PIXEL_HEIGHT - 1), (200, 200, 0)); // bottom-right is yellow

    host_buffer_offset
}

#[unsafe(no_mangle)]
pub extern "C" fn white(_ticks: u64, _frame: u64, host_buffer_offset: u32) -> u32 {
    // Use the host-provided buffer
    let ptr = host_buffer_offset as *mut u8;

    unsafe {
        core::ptr::write_bytes(ptr, 255, PIXEL_BUFFER_SIZE);
    }

    host_buffer_offset
}

#[unsafe(no_mangle)]
pub extern "C" fn rainbow_cycle(ticks: u64, _frame: u64, host_buffer_offset: u32) -> u32 {
    // Use the host-provided buffer
    let ptr = host_buffer_offset as *mut u8;

    // Time-based frame calculation
    let frame = ticks * FPS / TICKS_PER_SECOND;

    for y in 0..PIXEL_HEIGHT {
        for x in 0..PIXEL_WIDTH {
            // Diagonal rainbow: hue based on x + y + frame
            let hue = ((x + y) as u64 * 8 + frame * 2) % 256;

            // Simple HSV to RGB (S=1, V=1)
            let (r, g, b) = hsv_to_rgb(hue as u8);

            let offset = (y * PIXEL_WIDTH + x) * PIXEL_CHANNELS;
            unsafe {
                ptr.add(offset).write(r);
                ptr.add(offset + 1).write(g);
                ptr.add(offset + 2).write(b);
            }
        }
    }

    host_buffer_offset
}

// Simple HSV to RGB with S=1, V=1
fn hsv_to_rgb(hue: u8) -> (u8, u8, u8) {
    let h = hue as u16;
    let sector = h / 43; // 0-5 (256/6 ≈ 43)
    let offset = (h % 43) * 6; // Position within sector, scaled to 0-255

    match sector {
        0 => (255, offset as u8, 0),       // Red -> Yellow
        1 => (255 - offset as u8, 255, 0), // Yellow -> Green
        2 => (0, 255, offset as u8),       // Green -> Cyan
        3 => (0, 255 - offset as u8, 255), // Cyan -> Blue
        4 => (offset as u8, 0, 255),       // Blue -> Magenta
        _ => (255, 0, 255 - offset as u8), // Magenta -> Red
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn anim0001(ticks: u64, _frame: u64, _host_buffer_offset: u32) -> u32 {
    // Use our own (static) buffers
    // Scale down the frame number to control animation speed

    // Time-based frame calculation
    let frame = ticks * FPS / TICKS_PER_SECOND;

    // Slow the animation down
    let frame = frame / 16;

    let anim_frame = frame % ANIM_0001_IMAGE_DATA.len() as u64;
    let frame_data = ANIM_0001_IMAGE_DATA[anim_frame as usize];
    frame_data.as_ptr() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn proc0001(ticks: u64, _frame: u64, _host_buffer_offset: u32) -> u32 {
    // Use our own buffer
    let ptr = buffer_ptr();

    // Time-based frame calculation
    let frame = ticks * FPS / TICKS_PER_SECOND;

    for y in 0..PIXEL_HEIGHT {
        if y % 2 == 0 {
            continue;
        }
        for x in 0..PIXEL_WIDTH {
            if x % 2 == 0 {
                continue;
            }
            let hue = (x as u64 + frame) % 256;

            let (r, g, b) = hsv_to_rgb(hue as u8);

            let offset = (y * PIXEL_WIDTH + x) * PIXEL_CHANNELS;
            unsafe {
                ptr.add(offset).write(r);
                ptr.add(offset + 1).write(g);
                ptr.add(offset + 2).write(b);
            }
        }
    }

    ptr as u32
}

mod anim0002 {
    // A vertical sprite-sheet animation with 6 frames, each 16x16 pixels (768 bytes)
    pub(crate) static IMAGE_DATA: &[u8] = include_bytes!("../target/anim-0002.raw");
    include!(concat!(env!("OUT_DIR"), "/anim0002.rs"));
}

#[unsafe(no_mangle)]
pub extern "C" fn anim0002(ticks: u64, _frame: u64, _host_buffer_offset: u32) -> u32 {
    const TOTAL_TICKS: u64 = anim0002::FRAME_OFFSETS.last().unwrap().1;
    let ticks = ticks % TOTAL_TICKS;

    // Find the current frame based on the current cyclic tick count
    for (i, &(_, frame_end)) in anim0002::FRAME_OFFSETS.iter().enumerate() {
        if ticks < frame_end {
            let frame_start = anim0002::FRAME_OFFSETS[i].0;
            let frame_end = frame_start + PIXEL_BUFFER_SIZE;
            return anim0002::IMAGE_DATA[frame_start..frame_end].as_ptr() as u32;
        }
    }

    // If error, default to white frame
    white(ticks, _frame, _host_buffer_offset)

    // TODO: loops - need to switch to a state machine
}
