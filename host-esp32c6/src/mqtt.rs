use core::fmt::Write;
use embassy_futures::select::{select, Either};
use embassy_net::{tcp::TcpSocket, Ipv4Address, Stack};
use embassy_time::{Duration, Timer};
use rust_mqtt::client::event::{Event, Suback};
use rust_mqtt::client::options::{PublicationOptions, RetainHandling, SubscriptionOptions};
use rust_mqtt::types::{QoS, TopicName};
use rust_mqtt::{
    buffer::BumpBuffer,
    client::{
        options::{ConnectOptions, WillOptions},
        Client,
    },
    config::{KeepAlive, SessionExpiryInterval},
    types::{MqttBinary, MqttString},
    Bytes,
};

const BROKER_IP: Ipv4Address = Ipv4Address::new(192, 168, 1, 201);
const BROKER_PORT: u16 = 1883;

#[embassy_executor::task]
pub async fn mqtt_task(stack: Stack<'static>) {
    loop {
        if stack.is_config_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }
    defmt::info!("Network is up!");

    let mut rx_buffer = [0u8; 4096];
    let mut tx_buffer = [0u8; 4096];
    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(10)));

    let endpoint = (BROKER_IP, BROKER_PORT);
    if let Err(e) = socket.connect(endpoint).await {
        defmt::error!("TCP connect failed: {:?}", defmt::Debug2Format(&e));
        return;
    }
    defmt::info!("TCP connected");

    let mut buf = [0u8; 1024];
    let mut buffer = BumpBuffer::new(&mut buf);

    let mut client = Client::<'_, _, _, 1, 1, 1>::new(&mut buffer);

    let options = ConnectOptions {
        clean_start: true,
        session_expiry_interval: SessionExpiryInterval::Seconds(60),
        keep_alive: KeepAlive::Seconds(5),
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
            defmt::info!("Connected to MQTT broker: {:?}", c);
            defmt::info!("{:?}", client.client_config());
            defmt::info!("{:?}", client.server_config());
            defmt::info!("{:?}", client.shared_config());
            defmt::info!("{:?}", defmt::Debug2Format(&client.session()));
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

    let topic =
        unsafe { TopicName::new_unchecked(MqttString::from_slice("host-esp32c6/mbox").unwrap()) };

    match client.subscribe(topic.clone().into(), sub_options).await {
        Ok(_) => defmt::info!("Sent Subscribe"),
        Err(e) => {
            defmt::error!("Failed to subscribe: {:?}", e);
            return;
        }
    };

    match client.poll().await {
        Ok(Event::Suback(Suback {
            packet_identifier: _,
            reason_code,
        })) => defmt::info!("Subscribed with reason code {:?}", reason_code),
        Ok(e) => {
            defmt::error!("Expected Suback but received event {:?}", e);
            return;
        }
        Err(e) => {
            defmt::error!("Failed to receive Suback {:?}", e);
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
            defmt::info!("Published message with packet identifier {}", i);
            i
        }
        Err(e) => {
            defmt::error!("Failed to send Publish {:?}", e);
            return;
        }
    };

    let mut counter = 0;

    // Main loop: publish periodically + receive incoming messages
    loop {
        // Safe to reset here because we've finished processing any
        // previous poll_body data by this point in the loop.
        unsafe { client.buffer().reset() };

        match select(Timer::after(Duration::from_secs(5)), client.poll_header()).await {
            // Timer fired — publish an update
            Either::First(_) => {
                counter += 1;
                let mut message: heapless::String<64> = heapless::String::new();
                write!(message, "Update #{} from host-esp32c6", counter).unwrap();

                match client
                    .publish(&pub_options, Bytes::from(message.as_bytes()))
                    .await
                {
                    Ok(_) => defmt::info!("Published"),
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
                defmt::info!("Received header {:?}", h.packet_type());
                match client.poll_body(h).await {
                    Ok(Event::Publish(msg)) => {
                        defmt::info!(
                            "Received publish on topic, payload len={}",
                            msg.message.len()
                        );

                        // Process the message here
                        if let Ok(payload_str) = core::str::from_utf8(&msg.message) {
                            defmt::info!("Message payload: \"{}\"", payload_str);
                        } else {
                            defmt::info!(
                                "Message payload: {:?}",
                                defmt::Debug2Format(&msg.message)
                            );
                        }
                    }
                    Ok(e) => defmt::info!("Event: {:?}", e),
                    Err(e) => {
                        defmt::error!("poll_body failed: {:?}", e);
                        return;
                    }
                }
            }
        }
    }
}
