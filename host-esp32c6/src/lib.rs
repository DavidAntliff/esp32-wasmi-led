//#![cfg_attr(not(test), no_std)]
#![no_std]

pub mod mqtt;

// A macro that calls defmt::info!() as well as println!()
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        esp_println::println!($($arg)*);
    }};
}
