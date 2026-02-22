#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

extern crate alloc;

use alloc::collections::VecDeque;
use esp_hal::clock::CpuClock;
use esp_hal::rmt::Rmt;
use esp_hal::time::Instant;
use esp_hal::timer::timg::TimerGroup;
use esp_hal_smartled::{buffer_size, color_order, RmtSmartLeds, Ws2812Timing};
use esp_println::println;
use host_common::serpentine_index;
use smart_leds::{brightness, gamma, SmartLedsWrite, RGB8};
use wasmi::{Engine, Linker, Memory, Module, Store, TypedFunc};

#[allow(unused_imports)]
use esp_backtrace as _;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

const FPS_WINDOW_SIZE: usize = 60;
const TICKS_PER_SECOND: u64 = 256;
const BRIGHTNESS: u8 = 100;

// A macro that calls defmt::info!() as well as println!()
macro_rules! log {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        println!($($arg)*);
    }};
}

pub struct AppState {
    start_time: Instant,
    ticks: u64,
    counter: u64,
    frame_times: VecDeque<Instant>,
    guest_state: GuestState,
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

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    log!("🦀 WASM LED Matrix Host");

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    //esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);
    esp_alloc::heap_allocator!(size: 262144);

    // Initialise Embassy
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    // TODO: refactor

    // FIXME: import guest source into same project
    let wasm_bytes =
        include_bytes!("../../../guest/target/wasm32-unknown-unknown/release/guest.wasm");
    log!("Initialising engine...");
    let engine = Engine::default();
    log!("Initialising module...");
    let module = Module::new(&engine, wasm_bytes).expect("Failed to create module");
    log!("Initialising store...");
    let mut store = Store::new(&engine, ());
    log!("Initialising linker...");
    let linker = Linker::<()>::new(&engine);

    log!("Instantiating instance...");
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .expect("Failed to instantiate module");

    let memory = instance
        .get_memory(&store, "memory")
        .expect("Failed to get guest memory");

    let host_buffer_offset = memory.data(&store).len() as u32;
    println!("Host pixel buffer at offset 0x{host_buffer_offset:04x}");

    // Grow guest memory by 1 page (64KiB) to give some space for the host buffer
    memory.grow(&mut store, 1).expect("Failed to grow memory");
    log!(
        "Guest memory size: 0x{:04x} bytes",
        memory.data(&store).len()
    );

    assert!(
        host_buffer_offset as usize + 768 <= (memory.data_size(&store)),
        "Not enough memory for host pixel buffer"
    );

    log!("Fetching 'update' function...");
    let update_func = instance
        .get_typed_func::<(u64, u64, u32), u32>(&mut store, "update")
        .expect("Failed to get 'update' function");

    log!("Fetching 'init' function...");
    let init_func = instance
        .get_typed_func::<(), ()>(&mut store, "init")
        .expect("Failed to get 'init' function");

    log!("Calling 'init' function...");
    init_func
        .call(&mut store, ())
        .expect("Failed to call 'init' function");

    let mut app_state = AppState {
        start_time: Instant::now(),
        counter: 0,
        ticks: 0,
        frame_times: VecDeque::with_capacity(FPS_WINDOW_SIZE),
        guest_state: GuestState {
            _engine: engine,
            store,
            _linker: linker,
            memory,
            host_buffer_offset,
            _init: init_func,
            update: update_func,
        },
    };

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

    // Initialise LED hardware driver
    let led_pin = peripherals.GPIO10;
    let freq = esp_hal::time::Rate::from_mhz(80);
    type LedColor = RGB8;
    const HEIGHT: usize = 16;
    const WIDTH: usize = 16;
    const NUM_LEDS: usize = WIDTH * HEIGHT;
    let mut led = {
        let rmt = Rmt::new(peripherals.RMT, freq).expect("RMT should initialise");
        RmtSmartLeds::<
            { buffer_size::<LedColor>(NUM_LEDS) },
            _,
            LedColor,
            color_order::Grb,
            Ws2812Timing,
        >::new_with_memsize(rmt.channel0, led_pin, 2)
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

    let mut data: [RGB8; NUM_LEDS] = [RGB8::default(); NUM_LEDS];

    log!("Entering main loop...");
    loop {
        // 256 ticks per second (millisecond)
        let elapsed = Instant::now() - app_state.start_time;
        app_state.ticks = elapsed.as_millis() * TICKS_PER_SECOND / 1000;

        let pixel_buffer = app_state
            .guest_state
            .update
            .call(
                &mut app_state.guest_state.store,
                (
                    app_state.ticks,
                    app_state.counter,
                    app_state.guest_state.host_buffer_offset,
                ),
            )
            .expect("Failed to call 'update' function");

        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let src = y * WIDTH + x;
                let dst = serpentine_index(x, y, WIDTH, HEIGHT);
                //log!("({}, {}), src={}, dst={}", x, y, src, dst);

                let mut color_buf = [0u8; 3];
                let pixel_id = (src) * 3usize;
                let offset = pixel_buffer as usize + pixel_id;

                app_state
                    .guest_state
                    .memory
                    .read(&app_state.guest_state.store, offset, &mut color_buf)
                    .expect("Should read pixel buffer memory");

                data[dst] = RGB8 {
                    r: color_buf[0],
                    g: color_buf[1],
                    b: color_buf[2],
                };
            }
        }

        led.write(brightness(gamma(data.iter().cloned()), BRIGHTNESS))
            .expect("Should write to LED");

        app_state.counter += 1;

        app_state.frame_times.push_back(Instant::now());
        if app_state.frame_times.len() > FPS_WINDOW_SIZE {
            app_state.frame_times.pop_front();
        }

        // if app_state.counter % 100 == 0 {
        //     let fps = if app_state.frame_times.len() >= 2 {
        //         let oldest = app_state.frame_times.front().expect("Should be Some");
        //         let newest = app_state.frame_times.back().expect("Should be Some");
        //         let duration = (*newest - *oldest).as_millis() as f64 / 1000.0;
        //         log!("Duration {}", duration);
        //         log!("Frames {}", app_state.frame_times.len() - 1);
        //         if duration > 0.0 {
        //             (app_state.frame_times.len() - 1) as f64 / duration
        //         } else {
        //             log!("zero duration");
        //             0.0
        //         }
        //     } else {
        //         log!("not enough");
        //         0.0
        //     };
        //     log!("FPS: {}", fps);
        // }

        // Yield to let other tasks run
        embassy_time::Timer::after(embassy_time::Duration::from_millis(1)).await;
    }
}
