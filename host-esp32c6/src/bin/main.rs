#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

extern crate alloc;
use blinksy::color::ColorCorrection;
use blinksy::driver::Driver;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;
use esp_hal::clock::CpuClock;
use esp_hal::main;
use esp_hal::time::Instant;
use esp_println::println;
use panic_rtt_target as _;
use spin::Mutex;
use wasmi::{Engine, Linker, Module, Store};

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

// A macro that calls defmt::info!() as well as println!()
macro_rules! log {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        println!($($arg)*);
    }};
}

#[main]
fn main() -> ! {
    rtt_target::rtt_init_defmt!();

    log!("🦀 WASM Host Demo - Fibonacci Generator (wasmi Runtime)");

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    //esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);
    esp_alloc::heap_allocator!(size: 131072);

    let led_driver = led_matrix(peripherals);

    run_wasm(led_driver);

    loop {}
}

fn run_wasm(led_driver: impl Driver<Word = u8> + Send + Sync + 'static) {
    // 1. Embed the Wasm binary as a byte array
    let wasm_bytes = include_bytes!(
        "../../../guest-fibonacci/target/wasm32-unknown-unknown/release/guest_fibonacci.wasm"
    );
    //let wasm_bytes = include_bytes!("../../../guest-fibonacci/target/wasm32-unknown-unknown/release/guest_fibonacci_opt.wasm");
    //let wasm_bytes = include_bytes!("../../../guest-fibonacci/target/wasm32v1-none/release/guest_fibonacci.wasm");
    //let wasm_bytes = include_bytes!("../../wat/minimal.wasm");
    //let wasm_bytes = include_bytes!("../../wat/42.wasm");
    //let wasm_bytes = include_bytes!("../../wat/memory.wasm");

    // 2. Set up the wasmi engine and store
    log!("Initialising engine...");
    let engine = Engine::default();
    log!("Initialising module...");
    let module = Module::new(&engine, wasm_bytes).expect("Failed to create module");
    log!("Initialising store...");
    let mut store = Store::new(&engine, ());
    log!("Initialising linker...");
    let mut linker = Linker::<()>::new(&engine);

    let count = AtomicU32::new(0);

    // Define the host function that the WASM module can call
    linker
        .func_wrap("env", "output", move |num: u64| {
            let c = count.fetch_add(1, Ordering::Relaxed) + 1;
            if c % 10_000 == 0 {
                log!("-- Calculated {} Fibonacci numbers -- {}", c, num);
            }
        })
        .expect("Failed to define host function");

    const BUFFER_WIDTH: usize = 16;
    const BUFFER_HEIGHT: usize = 16;
    const NUM_PIXELS: usize = BUFFER_WIDTH * BUFFER_HEIGHT;
    const PIXEL_BUFFER_SIZE: usize = NUM_PIXELS * 3;

    let mut pixels: heapless::Vec<u8, { PIXEL_BUFFER_SIZE }> = heapless::Vec::new();
    pixels.resize(PIXEL_BUFFER_SIZE, 0).unwrap();
    let pixels = portable_atomic_util::Arc::new(Mutex::new(pixels));

    let pixels_clone = pixels.clone();
    linker
        .func_wrap(
            "env",
            "set_pixel",
            move |x: u32, y: u32, r: u32, g: u32, b: u32| {
                //log!("set_pixel called: x={}, y={}, value={}", x, y, value);
                let mut pixels = pixels_clone.lock();
                let i = 3 * ((y as usize) * BUFFER_WIDTH + (x as usize));
                pixels[i + 0] = g as u8; // G
                pixels[i + 1] = r as u8; // R
                pixels[i + 2] = b as u8; // B
            },
        )
        .expect("Failed to define host function");

    let pixels_clone = pixels.clone();
    linker
        .func_wrap("env", "fill", move |r: u32, g: u32, b: u32| {
            //log!("fill called: r={}, g={}, b={}", r, g, b);
            let mut pixels = pixels_clone.lock();
            for y in 0..BUFFER_HEIGHT {
                for x in 0..BUFFER_WIDTH {
                    let i = 3 * (y * BUFFER_WIDTH + x);
                    pixels[i + 0] = g as u8; // G
                    pixels[i + 1] = r as u8; // R
                    pixels[i + 2] = b as u8; // B
                }
            }
        })
        .expect("Failed to define host function");

    let led_driver = portable_atomic_util::Arc::new(Mutex::new(led_driver));
    let led_driver_clone = led_driver.clone();

    let pixels_clone = pixels.clone();
    linker
        .func_wrap("env", "update", move || {
            //log!("update called);
            let pixels = pixels_clone.lock();
            let mut driver = led_driver_clone.lock();
            let _ = driver.write(pixels.clone(), 1.0, ColorCorrection::default());
        })
        .expect("Failed to define host function");

    // 3. Instantiate the module
    log!("Instantiating instance...");
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .expect("Failed to instantiate module");

    // Get the 'fib' function from the WASM module
    log!("Fetching 'fib' function...");
    let fib_func = instance
        .get_typed_func::<u64, u64>(&mut store, "fib")
        .expect("Failed to get 'fib' function");

    log!("Fetching 'add' function...");
    let add_func = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "add")
        .expect("Failed to get 'add' function");

    log!("Fetching 'fill' function...");
    let fill_slow_func = instance
        .get_typed_func::<(u32, u32, u32, u32, u32), ()>(&mut store, "fill_slow")
        .expect("Failed to get 'fill_slow' function");

    log!("Fetching 'render' function...");
    let render_func = instance
        .get_typed_func::<(u32, u32, u32), ()>(&mut store, "render")
        .expect("Failed to get 'render' function");

    // Call the 'add' function as a quick test
    log!("Calling 'add' function...");
    let result = add_func
        .call(&mut store, (41, 1))
        .expect("Failed to call 'add' function");
    log!("add(41, 1) returned: {}", result);

    // // Call the 'fill' function to start the Fibonacci sequence
    // log!("Calling 'fill_slow' function...");
    // let start = Instant::now();
    // fill_slow_func.call(&mut store, (16, 16, 255, 0, 0)).expect("Failed to call 'fill_slow' function");
    // let duration = start.elapsed();
    // log!("fill_slow(16, 16, 255, 0, 0) returned: {}", duration);
    // //log!("{:?}", buffer);

    const TARGET_FRAMES: u32 = 300;

    // // Measure time to fill multiple times at 8x8
    // let start = Instant::now();
    // //render.call(&mut store, (8, 8)).expect("Failed to call 'render' function");
    // for i in 0..TARGET_FRAMES {
    //     fill_func.call(&mut store, (8, 8, i, i, i)).expect("Failed to call 'fill' function");
    // }
    // let duration = start.elapsed();
    // let fps = TARGET_FRAMES as f32 / (duration.as_micros() as f32 / 1_000_000.0);
    // log!("{TARGET_FRAMES} * fill(8, 8, _) took: {}, {} fps", duration, fps);

    // Measure time to fill multiple times at 16x16
    // let start = Instant::now();
    // for i in 0..TARGET_FRAMES {
    //     fill_slow_func.call(&mut store, (16, 16, i, 1024 - i, i)).expect("Failed to call 'fill_slow' function");
    // }
    // let duration = start.elapsed();
    // let fps = TARGET_FRAMES as f32 / (duration.as_micros() as f32 / 1_000_000.0);
    // log!("{TARGET_FRAMES} * fill(16, 16, _) took: {}, {} fps", duration, fps);

    // // Measure time to fill multiple times at 8x8
    // let start = Instant::now();
    // render_func.call(&mut store, (8, 8, TARGET_FRAMES)).expect("Failed to call 'render' function");
    // let duration = start.elapsed();
    // let fps = TARGET_FRAMES as f32 / (duration.as_micros() as f32 / 1_000_000.0);
    // log!("render(8, 8, {TARGET_FRAMES}) took: {}, {} fps", duration, fps);

    // Measure time to fill multiple times at 16x16
    let start = Instant::now();
    render_func
        .call(&mut store, (16, 16, TARGET_FRAMES))
        .expect("Failed to call 'render' function");
    let duration = start.elapsed();
    let fps = TARGET_FRAMES as f32 / (duration.as_micros() as f32 / 1_000_000.0);
    log!(
        "render(16, 16, {}) took: {}, {} fps",
        TARGET_FRAMES,
        duration,
        fps
    );

    // Call the 'fib' function to start the Fibonacci sequence
    log!("Calling 'fib' function...");
    let start = Instant::now();
    let result = fib_func
        .call(&mut store, 100_000)
        .expect("Failed to call 'fib' function");
    let duration = start.elapsed();
    log!("fib(100_000_000) returned: {}, {}", result, duration);
}

fn led_matrix(p: esp_hal::peripherals::Peripherals) -> impl Driver<Word = u8> {
    use blinksy::layout::Layout2d;

    blinksy::layout2d!(
        Layout,
        [blinksy::layout::Shape2d::Grid {
            start: blinksy::layout::Vec2::new(-1., -1.),
            horizontal_end: blinksy::layout::Vec2::new(1., -1.),
            vertical_end: blinksy::layout::Vec2::new(-1., 1.),
            horizontal_pixel_count: 16,
            vertical_pixel_count: 16,
            serpentine: true,
        }]
    );

    // Setup the WS2812 driver using RMT.
    let ws2812_driver = {
        // IMPORTANT: Change `p.GPIO8` to the GPIO pin connected to your WS2812 data line.
        let data_pin = p.GPIO8;

        // Initialize RMT peripheral (typical base clock 80 MHz).
        let rmt_clk_freq = esp_hal::time::Rate::from_mhz(80);
        let rmt = esp_hal::rmt::Rmt::new(p.RMT, rmt_clk_freq).unwrap();
        let rmt_channel = rmt.channel0;

        // Create the driver using the ClocklessRmt builder.
        blinksy::driver::ClocklessDriver::default()
            .with_led::<blinksy::leds::Ws2812>()
            .with_writer(
                blinksy_esp::ClocklessRmtBuilder::default()
                    .with_rmt_buffer_size::<{ Layout::PIXEL_COUNT * 3 * 8 + 1 }>()
                    .with_led::<blinksy::leds::Ws2812>()
                    .with_channel(rmt_channel)
                    .with_pin(data_pin)
                    .build(),
            )
    };

    // // Build the Blinky controller
    // let mut control = blinksy::ControlBuilder::new_2d()
    //     .with_layout::<Layout, { Layout::PIXEL_COUNT }>()
    //     .with_pattern::<blinksy::patterns::rainbow::Rainbow>(blinksy::patterns::rainbow::RainbowParams {
    //         ..Default::default()
    //     })
    //     .with_driver(ws2812_driver)
    //     .with_frame_buffer_size::<{ blinksy::leds::Ws2812::frame_buffer_size(Layout::PIXEL_COUNT) }>()
    //     .build();
    //
    // control.set_brightness(0.2); // Set initial brightness (0.0 to 1.0)
    //
    // loop {
    //     let elapsed_in_ms = blinksy_esp::time::elapsed().as_millis();
    //     control.tick(elapsed_in_ms).unwrap();
    // }

    // TODO: maybe we create a custom Pattern instead, have the wasm modify it, and let the Control
    //       handle the driver writing?

    // RED TEST PIXEL:
    // let mut buffer: heapless::Vec<u8, { 256 * 3 }> = heapless::Vec::new();
    // buffer.resize(256 * 3, 0).unwrap();
    //
    // buffer[0] = 0;    // G
    // buffer[1] = 255;  // R
    // buffer[2] = 0;    // B
    //
    // ws2812_driver.write(buffer, 1.0, ColorCorrection::default()).unwrap();

    ws2812_driver
}
