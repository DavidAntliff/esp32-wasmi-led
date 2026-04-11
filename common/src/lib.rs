#![cfg_attr(not(test), no_std)]

// LED panel dimensions
pub const LED_PANEL_HEIGHT: usize = 16;
pub const LED_PANEL_WIDTH: usize = 16;
pub const LED_PANEL_NUM_LEDS: usize = LED_PANEL_WIDTH * LED_PANEL_HEIGHT;

// 3 bytes per LED (RGB)
pub const BYTES_PER_LED: usize = 3;

pub const LED_BUFFER_SIZE: usize = LED_PANEL_NUM_LEDS * BYTES_PER_LED;

#[inline(always)]
pub fn led_offset(x: usize, y: usize) -> usize {
    (y * LED_PANEL_WIDTH + x) * BYTES_PER_LED
}

/// # SAFETY
/// The buffer must point to a valid, writeable [u8; LED_BUFFER_SIZE].
#[inline(always)]
pub unsafe fn set_color(buffer: *mut u8, (x, y): (usize, usize), (r, g, b): (u8, u8, u8)) {
    if x >= LED_PANEL_WIDTH || y >= LED_PANEL_HEIGHT {
        return; // Out of bounds
    }
    let offset = led_offset(x, y);
    unsafe {
        buffer.add(offset).write(r);
        buffer.add(offset + 1).write(g);
        buffer.add(offset + 2).write(b);
    }
}

/// # SAFETY
/// The buffer must point to a valid, writeable [u8; LED_BUFFER_SIZE].
pub unsafe fn set_all(buffer: *mut u8, color: (u8, u8, u8)) {
    // TODO: optimize with memset?
    for y in 0..LED_PANEL_HEIGHT {
        for x in 0..LED_PANEL_WIDTH {
            unsafe { set_color(buffer, (x, y), color) };
        }
    }
}
