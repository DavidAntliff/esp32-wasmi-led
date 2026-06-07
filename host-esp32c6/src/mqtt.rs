// This seems to fix RustRover's External Linter issue:
//   https://youtrack.jetbrains.com/issue/RUST-19797/False-external-linter-clippy-warnings-in-nostd-esp32-project
//#![cfg(not(test))]

use crate::{Command, DIRECT_CMD, MODE, log};
use core::fmt::Write;
use embassy_futures::select::{Either, select};
use embassy_net::{Ipv4Address, Stack, tcp::TcpSocket};
use embassy_time::{Duration, Ticker, Timer};
use rust_mqtt::client::event::{Event, Suback};
use rust_mqtt::client::options::{PublicationOptions, RetainHandling, SubscriptionOptions};
use rust_mqtt::types::{QoS, TopicName};
use rust_mqtt::{
    Bytes,
    buffer::BumpBuffer,
    client::{
        Client,
        options::{ConnectOptions, WillOptions},
    },
    config::{KeepAlive, SessionExpiryInterval},
    types::{MqttBinary, MqttString},
};

const BROKER_IP: Ipv4Address = Ipv4Address::new(192, 168, 1, 201);
const BROKER_PORT: u16 = 1883;

// Inbound control commands (JSON `Command`).
const MBOX_TOPIC: &str = "host-esp32c6/mbox";
// Ping request/response bridged by the axum backend. The prefix must match the
// backend's `DEFAULT_PREFIX` (`web-common`/`backend`).
const PING_REQ_TOPIC: &str = "esp32-wasmi-led/ping/request";
const PING_RESP_TOPIC: &str = "esp32-wasmi-led/ping/response";

/// Ping request published by the backend on [`PING_REQ_TOPIC`]. Matches the
/// backend's `PingPayload` JSON shape (`{correlation_id, message}`); we echo the
/// `correlation_id` back in the pong.
#[derive(serde::Deserialize)]
struct PingRequest {
    correlation_id: heapless::String<64>,
    // Always "ping"; parsed so there are no unknown fields, but unused.
    #[allow(dead_code)]
    message: heapless::String<32>,
}

#[embassy_executor::task]
pub async fn mqtt_task(stack: Stack<'static>) {
    log!("🌱 Start MQTT task...");

    loop {
        if stack.is_config_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
    log!("Network is up!");

    let mut rx_buffer = [0u8; 4096];
    let mut tx_buffer = [0u8; 4096];
    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(10)));

    let endpoint = (BROKER_IP, BROKER_PORT);
    if let Err(e) = socket.connect(endpoint).await {
        defmt::error!("TCP connect failed: {:?}", defmt::Debug2Format(&e));
        return;
    }
    log!("TCP connected");

    let mut buf = [0u8; 1024];
    let mut buffer = BumpBuffer::new(&mut buf);

    let mut client = Client::<'_, _, _, 1, 1, 1>::new(&mut buffer);

    let options = ConnectOptions {
        clean_start: true,
        session_expiry_interval: SessionExpiryInterval::Seconds(60),
        keep_alive: KeepAlive::Seconds(30 /*5*/),
        user_name: Some(MqttString::try_from("testUser").unwrap()),
        password: Some(MqttBinary::try_from("testPass").unwrap()),
        will: Some(WillOptions {
            will_qos: QoS::ExactlyOnce,
            will_retain: true,
            will_topic: MqttString::try_from("i/am/dead").unwrap(),
            will_payload: MqttBinary::try_from("Have a nice day!").unwrap(),
            will_delay_interval: 1,
            is_payload_utf8: true,
            message_expiry_interval: None,
            content_type: Some(MqttString::try_from("txt").unwrap()),
            response_topic: None,
            correlation_data: None,
        }),
    };

    // socket goes straight in — it already impls embedded_io_async::{Read, Write}
    match client
        .connect(
            socket,
            &options,
            Some(MqttString::try_from("rust-mqtt-demo-client").unwrap()),
        )
        .await
    {
        Ok(c) => {
            log!("Connected to MQTT broker: {:?}", c);
            log!("{:?}", client.client_config());
            log!("{:?}", client.server_config());
            log!("{:?}", client.shared_config());
            log!("{:?}", defmt::Debug2Format(&client.session()));
        }
        Err(e) => {
            defmt::error!("MQTT connect failed: {:?}", e);
            return;
        }
    }

    // Remember to reset the bump buffer between operations:
    // unsafe { client.buffer().reset() };

    // Subscribe
    let sub_options = SubscriptionOptions {
        retain_handling: RetainHandling::SendIfNotSubscribedBefore,
        retain_as_published: true,
        no_local: false,
        //qos: QoS::ExactlyOnce,
        qos: QoS::AtMostOnce,
    };

    let topic = unsafe { TopicName::new_unchecked(MqttString::from_slice(MBOX_TOPIC).unwrap()) };

    match client.subscribe(topic.clone().into(), sub_options).await {
        Ok(_) => log!("Sent Subscribe"),
        Err(e) => {
            defmt::error!("Failed to subscribe: {:?}", e);
            return;
        }
    };

    match client.poll().await {
        Ok(Event::Suback(Suback {
            packet_identifier: _,
            reason_code,
        })) => log!("Subscribed with reason code {:?}", reason_code),
        Ok(e) => {
            defmt::error!("Expected Suback but received event {:?}", e);
            return;
        }
        Err(e) => {
            defmt::error!("Failed to receive Suback {:?}", e);
            return;
        }
    }

    // Also subscribe to the ping-request topic so we can answer the backend's pings.
    // Sequential (subscribe -> wait for Suback) keeps the client's MAX_SUBSCRIBES=1
    // in-flight bound satisfied. `sub_options` is `Copy`, so it can be reused.
    let ping_topic =
        unsafe { TopicName::new_unchecked(MqttString::from_slice(PING_REQ_TOPIC).unwrap()) };

    match client
        .subscribe(ping_topic.clone().into(), sub_options)
        .await
    {
        Ok(_) => log!("Sent Subscribe (ping)"),
        Err(e) => {
            defmt::error!("Failed to subscribe (ping): {:?}", e);
            return;
        }
    };

    match client.poll().await {
        Ok(Event::Suback(Suback {
            packet_identifier: _,
            reason_code,
        })) => log!("Subscribed (ping) with reason code {:?}", reason_code),
        Ok(e) => {
            defmt::error!("Expected Suback (ping) but received event {:?}", e);
            return;
        }
        Err(e) => {
            defmt::error!("Failed to receive Suback (ping) {:?}", e);
            return;
        }
    }

    // Say hello
    let topic = unsafe { TopicName::new_unchecked(MqttString::from_slice("test").unwrap()) };

    let pub_options = PublicationOptions {
        retain: false,
        topic: topic.clone(),
        //qos: QoS::ExactlyOnce,
        qos: QoS::AtMostOnce,
    };

    let _publish_packet_id = match client
        .publish(
            &pub_options,
            Bytes::from("Hello from host-esp32c6".as_bytes()),
        )
        .await
    {
        Ok(i) => {
            log!("Published message with packet identifier {}", i);
            i
        }
        Err(e) => {
            defmt::error!("Failed to send Publish {:?}", e);
            return;
        }
    };

    let mut counter = 0;
    //let mut ticker = Ticker::every(Duration::from_secs(5));
    let mut ticker = Ticker::every(Duration::from_millis(5000));

    // Main loop: publish periodically + receive incoming messages
    loop {
        // Safe to reset here because we've finished processing any
        // previous poll_body data by this point in the loop.
        unsafe { client.buffer().reset() };

        match select(ticker.next(), client.poll_header()).await {
            // Timer fired — publish an update
            Either::First(_) => {
                counter += 1;
                let mut message: heapless::String<64> = heapless::String::new();
                write!(message, "Update #{} from host-esp32c6", counter).unwrap();

                match client
                    .publish(&pub_options, Bytes::from(message.as_bytes()))
                    .await
                {
                    Ok(_) => {} //log!("Published"),
                    Err(e) => {
                        defmt::error!("Publish failed: {:?}", e);
                        return;
                    }
                }
            }

            // Incoming packet header received — read the body
            Either::Second(header_result) => {
                let h = match header_result {
                    Ok(h) => h,
                    Err(e) => {
                        defmt::error!("poll_header failed: {:?}", e);
                        return;
                    }
                };
                log!("Received header {:?}", h.packet_type());

                // Built inside the Publish arm below, then published after the `msg`
                // borrow of `client` is released (publish needs `&mut client`).
                let mut pending_pong: Option<heapless::String<128>> = None;

                match client.poll_body(h).await {
                    Ok(Event::Publish(msg)) => {
                        let topic: &str = msg.topic.as_ref();
                        log!(
                            "Received publish on '{}', payload len={}",
                            topic,
                            msg.message.len()
                        );

                        if topic == PING_REQ_TOPIC {
                            match serde_json_core::from_slice::<PingRequest>(&msg.message) {
                                Ok((req, _)) => {
                                    let mut p: heapless::String<128> = heapless::String::new();
                                    match write!(
                                        p,
                                        "{{\"correlation_id\":\"{}\",\"message\":\"pong from host-esp32c6\"}}",
                                        req.correlation_id
                                    ) {
                                        Ok(()) => pending_pong = Some(p),
                                        Err(_) => defmt::warn!("Ping response payload too long"),
                                    }
                                }
                                Err(e) => defmt::warn!(
                                    "Failed to parse ping request: {:?}",
                                    defmt::Debug2Format(&e)
                                ),
                            }
                        } else if topic == MBOX_TOPIC {
                            match serde_json_core::from_slice::<Command>(&msg.message) {
                                Ok((command, _bytes_consumed)) => {
                                    log!("Parsed command: {:?}", command);
                                    dispatch_command(command).await;
                                }
                                Err(e) => {
                                    // Log what we received for debugging
                                    if let Ok(s) = core::str::from_utf8(&msg.message) {
                                        defmt::warn!(
                                            "Failed to parse: \"{}\" err={:?}",
                                            s,
                                            defmt::Debug2Format(&e)
                                        );
                                    } else {
                                        defmt::warn!(
                                            "Failed to parse non-UTF8 payload, err={:?}",
                                            defmt::Debug2Format(&e)
                                        );
                                    }
                                }
                            }
                        } else {
                            defmt::warn!("Publish on unexpected topic: {}", topic);
                        }
                    }
                    Ok(e) => log!("Event: {:?}", e),
                    Err(e) => {
                        defmt::error!("poll_body failed: {:?}", e);
                        return;
                    }
                }

                // The `msg` borrow is released here, so it's safe to publish the pong.
                if let Some(payload) = pending_pong {
                    let resp_topic = unsafe {
                        TopicName::new_unchecked(MqttString::from_slice(PING_RESP_TOPIC).unwrap())
                    };
                    let resp_options = PublicationOptions {
                        retain: false,
                        topic: resp_topic,
                        qos: QoS::AtMostOnce,
                    };
                    match client
                        .publish(&resp_options, Bytes::from(payload.as_bytes()))
                        .await
                    {
                        Ok(_) => log!("Published pong to {}", PING_RESP_TOPIC),
                        Err(e) => defmt::error!("Failed to publish pong: {:?}", e),
                    }
                }
            }
        }
    }
}

async fn dispatch_command(cmd: Command) {
    log!("dispatch_command: {:?}", cmd);
    match cmd {
        Command::SetMode(mode) => {
            MODE.sender().send(mode);
        }

        Command::DirectCommand(cmd) => {
            DIRECT_CMD.sender().send(cmd).await;
        }
    }
}
