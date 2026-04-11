//#![cfg_attr(not(test), no_std)]
#![no_std]

use core::sync::atomic::AtomicUsize;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use serde::{Deserialize, Serialize};

pub mod direct;
pub mod led;
pub mod mqtt;
pub mod net;
pub mod wasm;

// buffer provider signals this when a frame is ready in the pixel buffer
pub(crate) static FRAME_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
// led_task signals this when it's done reading the pixel buffer
pub(crate) static FRAME_CONSUMED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// Pointer to the current frame's pixel buffer (set by buffer provider task before signalling FRAME_READY)
pub(crate) static FRAME_PTR: AtomicUsize = AtomicUsize::new(0);
// Length of the pixel data in bytes
pub(crate) static FRAME_LEN: AtomicUsize = AtomicUsize::new(0);

// Broadcast the current mode to all listening tasks
pub static MODE: Watch<CriticalSectionRawMutex, Mode, 2> = Watch::new();

// Host pixel buffer pointer, created within WASM guest memory space.
// Valid after WASM init; backed by wasmi heap memory that outlives all tasks.
pub(crate) static HOST_BUFFER_PTR: AtomicUsize = AtomicUsize::new(0);

pub(crate) static DIRECT_CMD: Channel<CriticalSectionRawMutex, DirectCommand, 4> = Channel::new();

// A macro that calls defmt::info!() as well as println!()
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        esp_println::println!($($arg)*);
    }};
}

#[derive(Serialize, Deserialize, defmt::Format, Debug, Clone, PartialEq, Default)]
pub enum Mode {
    Direct,
    #[default]
    Wasm,
}

#[derive(Serialize, Deserialize, defmt::Format, Debug)]
pub(crate) struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

/// Top-left is {x: 0, y: 0}, bottom right is {x: NUM_X - 1, y: NUM_Y - 1}
#[derive(Serialize, Deserialize, defmt::Format, Debug)]
pub(crate) struct Point {
    x: u8,
    y: u8,
}

#[derive(Serialize, Deserialize, defmt::Format, Debug)]
pub(crate) enum DirectCommand {
    SetPixel { point: Point, color: Rgb },
    SetAll { color: Rgb },
}

#[derive(Serialize, Deserialize, defmt::Format, Debug)]
pub(crate) enum Command {
    SetMode(Mode),
    DirectCommand(DirectCommand),
}
