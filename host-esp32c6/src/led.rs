use crate::{log, FRAME_CONSUMED, FRAME_LEN, FRAME_PTR, FRAME_READY};
use common::{LED_PANEL_HEIGHT, LED_PANEL_NUM_LEDS, LED_PANEL_WIDTH};
use core::sync::atomic::Ordering;
use esp_hal::rmt::Rmt;
use esp_hal_smartled::{buffer_size, color_order, RmtSmartLeds, Ws2812Timing};
use host_common::serpentine_index;
use smart_leds::SmartLedsWrite;
use smart_leds::{brightness, gamma, RGB8};

const BRIGHTNESS: u8 = 100;

#[embassy_executor::task]
pub async fn led_task(
    gpio: esp_hal::gpio::AnyPin<'static>,
    rmt: esp_hal::peripherals::RMT<'static>,
) {
    log!("🌱 Start LED task...");

    // LED panel is a strip of 256 WS2812B LEDs arranged in a 16x16 grid, in a serpentine pattern.
    //
    // The first strip LED is at the panel's bottom left corner, then the sequence goes right,
    // then up a row, then goes left, then up a row, and so on in a serpentine pattern.
    // Therefore, the top left corner is the last LED at strip position 255.
    //
    //   255 254 253 252 251 250 249 248 247 246 245 244 243 242 241 240
    //   224 225 226 227 228 229 230 231 232 233 234 235 236 237 238 239
    //   223 ...
    //   ...
    //    32 ...
    //    31  30  29  28  27  26  25  24  23  22  21  20  19  18  17  16
    //     0   1   2   3   4   5   6   7   8   9  10  11  12  13  14  15

    let freq = esp_hal::time::Rate::from_mhz(80);
    type LedColor = RGB8;

    let mut led = {
        let rmt = Rmt::new(rmt, freq).expect("RMT should initialise");
        RmtSmartLeds::<
            { buffer_size::<LedColor>(LED_PANEL_NUM_LEDS) },
            _,
            LedColor,
            color_order::Grb,
            Ws2812Timing,
        >::new_with_memsize(rmt.channel0, gpio, 4) // memsize 2 is glitchy
        .expect("Should init LED driver")
    };

    // Clear all
    // let mut data = [RGB8::default(); NUM_LEDS];
    //
    // // Set strip index 0 to red
    // // data[0] = RGB8 { r: 255, g: 0, b: 0 };
    // // data[1] = RGB8 { r: 0, g: 255, b: 0 };
    // // data[15] = RGB8 { r: 0, g: 0, b: 255 };
    // // data[16] = RGB8 {
    // //     r: 200,
    // //     g: 200,
    // //     b: 0,
    // // };
    // data[240] = RGB8 { r: 255, g: 0, b: 0 };
    //
    // led.write(brightness(gamma(data.iter().cloned()), BRIGHTNESS))
    //     .expect("Should write to LED");
    //
    // loop {}

    let mut data = [RGB8::default(); LED_PANEL_NUM_LEDS];

    log!("🔁 LED task waiting for frames...");
    loop {
        FRAME_READY.wait().await;

        let ptr = FRAME_PTR.load(Ordering::Acquire);
        let len = FRAME_LEN.load(Ordering::Acquire);

        // SAFETY: wasmi_task is blocked on FRAME_CONSUMED, so the backing WASM memory is not
        // mutated. The pointer and length were validated by wasmi_task before signalling.
        let pixels: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len) };

        for y in 0..LED_PANEL_HEIGHT {
            for x in 0..LED_PANEL_WIDTH {
                let src = (y * LED_PANEL_WIDTH + x) * 3usize;
                let dst = serpentine_index(x, y, LED_PANEL_WIDTH, LED_PANEL_HEIGHT);
                data[dst] = RGB8 {
                    r: pixels[src],
                    g: pixels[src + 1],
                    b: pixels[src + 2],
                };
            }
        }

        // Disable interrupts to avoid glitches
        critical_section::with(|_| {
            led.write(brightness(gamma(data.iter().cloned()), BRIGHTNESS))
                .expect("Should write to LED");
        });

        FRAME_CONSUMED.signal(());
    }
}
