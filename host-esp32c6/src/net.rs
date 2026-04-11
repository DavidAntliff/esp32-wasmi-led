use crate::log;
use embassy_net::Runner;
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    ClientConfig, ModeConfig, ScanConfig, WifiController, WifiDevice, WifiEvent, WifiStaState,
};

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASSWORD");

#[embassy_executor::task]
pub async fn connection(mut controller: WifiController<'static>) {
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
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}
