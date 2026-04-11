#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

extern crate alloc;

use embassy_net::StackResources;
use embassy_time::{Duration, Timer};
#[allow(unused_imports)]
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::Controller;
use host_esp32c6::direct::direct_task;
use host_esp32c6::led::led_task;
use host_esp32c6::log;
use host_esp32c6::mqtt::mqtt_task;
use host_esp32c6::net::{connection, net_task};
use host_esp32c6::wasm::wasm_task;
use host_esp32c6::{Mode, MODE};

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

// When you are okay with using a nightly compiler it's better to use https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

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

    MODE.sender().send(Mode::Wasm);

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

    spawner.spawn(mqtt_task(stack)).ok();

    spawner.spawn(wasm_task()).ok();
    spawner.spawn(direct_task()).ok();

    spawner
        .spawn(led_task(peripherals.GPIO10.into(), peripherals.RMT))
        .ok();

    loop {
        Timer::after(Duration::from_millis(1_000)).await;
    }
}
