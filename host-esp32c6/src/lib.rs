//#![cfg_attr(not(test), no_std)]
#![no_std]

use core::sync::atomic::AtomicUsize;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_hal::time::Instant;
use wasmi::{Engine, Linker, Memory, Store, TypedFunc};

pub mod led;
pub mod mqtt;
pub mod net;
pub mod wasm;

// wasm_task signals this when a frame is ready in the pixel buffer
pub(crate) static FRAME_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
// led_task signals this when it's done reading the pixel buffer
pub(crate) static FRAME_CONSUMED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// Pointer to the current frame's pixel buffer (set by wasmi_task before signalling FRAME_READY)
pub(crate) static FRAME_PTR: AtomicUsize = AtomicUsize::new(0);
// Length of the pixel data in bytes
pub(crate) static FRAME_LEN: AtomicUsize = AtomicUsize::new(0);

pub struct AppState {
    start_time: Instant,
    ticks: u64,
    counter: u64,
}

pub struct GuestState {
    _engine: Engine,
    store: Store<()>,
    _linker: Linker<()>,
    memory: Memory,
    host_buffer_offset: u32,

    // Guest exports
    _init: TypedFunc<(), ()>,
    update: TypedFunc<(u64, u64, u32), u32>,
}

// A macro that calls defmt::info!() as well as println!()
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        esp_println::println!($($arg)*);
    }};
}
