#![cfg_attr(not(test), no_std)]

// LED panel dimensions
pub const LED_PANEL_HEIGHT: usize = 16;
pub const LED_PANEL_WIDTH: usize = 16;
pub const LED_PANEL_NUM_LEDS: usize = LED_PANEL_WIDTH * LED_PANEL_HEIGHT;
