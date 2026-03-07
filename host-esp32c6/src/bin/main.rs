#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use embassy_net::{Runner, StackResources};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
#[allow(unused_imports)]
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rmt::Rmt;
use esp_hal::rng::Rng;
use esp_hal::time::Instant;
use esp_hal::timer::timg::TimerGroup;
use esp_hal_smartled::{buffer_size, color_order, RmtSmartLeds, Ws2812Timing};
use esp_radio::{
    wifi::{
        ClientConfig, ModeConfig, ScanConfig, WifiController, WifiDevice, WifiEvent, WifiStaState,
    },
    Controller,
};
use host_common::serpentine_index;
use host_esp32c6::log;
use smart_leds::{brightness, gamma, SmartLedsWrite, RGB8};
use wasmi::{Engine, Linker, Memory, Module, Store, TypedFunc};

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

const TICKS_PER_SECOND: u64 = 256;
const BRIGHTNESS: u8 = 100;

// Pixel buffer dimensions
const HEIGHT: usize = 16;
const WIDTH: usize = 16;
const NUM_LEDS: usize = WIDTH * HEIGHT;

// wasmi_task signals this when a frame is ready in the pixel buffer
static FRAME_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
// led_task signals this when it's done reading the pixel buffer
static FRAME_CONSUMED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// Pointer to the current frame's pixel buffer (set by wasmi_task before signalling FRAME_READY)
static FRAME_PTR: AtomicUsize = AtomicUsize::new(0);
// Length of the pixel data in bytes
static FRAME_LEN: AtomicUsize = AtomicUsize::new(0);

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

// When you are okay with using a nightly compiler it's better to use https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASSWORD");

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    rtt_target::rtt_init_defmt!();

    log!("🦀 WASM LED Matrix Host");

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);
    esp_alloc::heap_allocator!(size: 262144);

    // Initialise Embassy
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    log!("🛜 Initialising WiFi...");
    let esp_radio_ctrl = &*mk_static!(Controller<'static>, esp_radio::init().unwrap());

    let (controller, interfaces) =
        esp_radio::wifi::new(&esp_radio_ctrl, peripherals.WIFI, Default::default()).unwrap();

    let wifi_interface = interfaces.sta;

    // DHCP
    //let config = embassy_net::Config::dhcpv4(Default::default());

    // Static IP
    let config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(embassy_net::Ipv4Address::new(192, 168, 1, 242), 24),
        gateway: Some(embassy_net::Ipv4Address::new(192, 168, 1, 1)),
        dns_servers: {
            let mut dns = heapless::Vec::<embassy_net::Ipv4Address, 3>::new();
            dns.push(embassy_net::Ipv4Address::new(192, 168, 1, 1))
                .unwrap();
            dns
        },
        // dns_servers: heapless::Vec::from_slice(&[embassy_net::Ipv4Address::new(192, 168, 1, 1)])
        //     .unwrap(),
    });

    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    // Init network stack
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(runner)).ok();

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    log!("🌐 Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            log!("🌐 Got IP: {}", defmt::Display2Format(&config.address));
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    spawner.spawn(host_esp32c6::mqtt::mqtt_task(stack)).ok();

    spawner.spawn(wasmi_task()).ok();

    spawner
        .spawn(led_task(peripherals.GPIO10.into(), peripherals.RMT))
        .ok();

    loop {
        Timer::after(Duration::from_millis(1_000)).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    log!("🌱 Start connection task...");
    log!(
        "Device capabilities: {:?}",
        defmt::Debug2Format(&controller.capabilities())
    );
    loop {
        if esp_radio::wifi::sta_state() == WifiStaState::Connected {
            // wait until we're no longer connected
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            log!("💀 WiFi disconnected");
            Timer::after(Duration::from_millis(5000)).await
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(SSID.into())
                    .with_password(PASSWORD.into()),
            );
            controller.set_config(&client_config).unwrap();
            log!("Starting wifi");
            controller.start_async().await.unwrap();
            log!("Wifi started!");

            log!("Scan");
            let scan_config = ScanConfig::default().with_max(10);
            let result = controller
                .scan_with_config_async(scan_config)
                .await
                .unwrap();
            for ap in result {
                log!("{:?}", defmt::Debug2Format(&ap));
            }
        }
        log!("🌐 About to connect...");

        match controller.connect_async().await {
            Ok(_) => log!("💀 Wifi connected!"),
            Err(e) => {
                log!(
                    "💀 Failed to connect to wifi: {:?}",
                    defmt::Debug2Format(&e)
                );
                Timer::after(Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
async fn wasmi_task() {
    log!("🌱 Start WASMI task...");

    let wasm_bytes =
        include_bytes!("../../../guest/target/wasm32-unknown-unknown/release/guest.wasm");
    log!("⚙️ Initialising WASMI engine...");
    let engine = Engine::default();
    log!("⚙️ Initialising WASMI module...");
    let module = Module::new(&engine, wasm_bytes).expect("Failed to create module");
    log!("⚙️ Initialising WASMI store...");
    let mut store = Store::new(&engine, ());
    log!("⚙️ Initialising WASMI linker...");
    let linker = Linker::<()>::new(&engine);

    log!("⚙️ Instantiating WASMI instance...");
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .expect("Failed to instantiate module");

    let memory = instance
        .get_memory(&store, "memory")
        .expect("Failed to get guest memory");

    let host_buffer_offset = memory.data(&store).len() as u32;

    // Grow guest memory by 1 page (64KiB) to give some space for the host buffer
    memory.grow(&mut store, 1).expect("Failed to grow memory");
    log!(
        "⚙️ Guest memory size: 0x{:04x} bytes @ offset 0x{:04x}",
        memory.data(&store).len(),
        host_buffer_offset
    );

    assert!(
        host_buffer_offset as usize + NUM_LEDS * 3 <= (memory.data_size(&store)),
        "Not enough memory for host pixel buffer"
    );

    let update_func = instance
        .get_typed_func::<(u64, u64, u32), u32>(&mut store, "update")
        .expect("Failed to get 'update' function");

    let init_func = instance
        .get_typed_func::<(), ()>(&mut store, "init")
        .expect("Failed to get 'init' function");

    log!("🧳 Calling guest 'init' function...");
    init_func
        .call(&mut store, ())
        .expect("Failed to call guest 'init' function");

    let mut guest_state = GuestState {
        _engine: engine,
        store,
        _linker: linker,
        memory,
        host_buffer_offset,
        _init: init_func,
        update: update_func,
    };

    let mut app_state = AppState {
        start_time: Instant::now(),
        ticks: 0,
        counter: 0,
    };

    log!("🔁 WASMI entering main loop...");
    loop {
        let elapsed = Instant::now() - app_state.start_time;
        app_state.ticks = elapsed.as_millis() * TICKS_PER_SECOND / 1000;

        let pixel_buffer = guest_state
            .update
            .call(
                &mut guest_state.store,
                (
                    app_state.ticks,
                    app_state.counter,
                    guest_state.host_buffer_offset,
                ),
            )
            .expect("Failed to call 'update' function");

        // Get a raw pointer to the pixel data inside WASM linear memory
        let mem_data = guest_state.memory.data(&guest_state.store);
        let offset = pixel_buffer as usize;
        let len = NUM_LEDS * 3;
        assert!(offset + len <= mem_data.len(), "pixel buffer out of bounds");

        let ptr = mem_data[offset..].as_ptr() as usize;

        // Publish the pointer — safe because led_task won't read until signalled,
        // and we block until it's done.
        FRAME_PTR.store(ptr, Ordering::Release);
        FRAME_LEN.store(len, Ordering::Release);

        FRAME_READY.signal(());
        FRAME_CONSUMED.wait().await;

        app_state.counter += 1;
        Timer::after(Duration::from_millis(1)).await;
    }
}

#[embassy_executor::task]
async fn led_task(gpio: esp_hal::gpio::AnyPin<'static>, rmt: esp_hal::peripherals::RMT<'static>) {
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
            { buffer_size::<LedColor>(NUM_LEDS) },
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

    let mut data = [RGB8::default(); NUM_LEDS];

    log!("🔁 LED task waiting for frames...");
    loop {
        FRAME_READY.wait().await;

        let ptr = FRAME_PTR.load(Ordering::Acquire);
        let len = FRAME_LEN.load(Ordering::Acquire);

        // SAFETY: wasmi_task is blocked on FRAME_CONSUMED, so the backing WASM memory is not
        // mutated. The pointer and length were validated by wasmi_task before signalling.
        let pixels: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len) };

        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let src = (y * WIDTH + x) * 3usize;
                let dst = serpentine_index(x, y, WIDTH, HEIGHT);
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
