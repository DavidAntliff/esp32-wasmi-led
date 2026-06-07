//! Integration tests for the egui-axum-mqtt backend.
//!
//! **Prerequisites**: a Mosquitto (or compatible) MQTT broker on localhost:1883
//! with `allow_anonymous true`.
//!
//! Run with:
//!   cargo test -p backend --test integration -- --nocapture

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite;
use web_common::{ClientMsg, LastMessage, ServerMsg};

use backend::{PingPayload, Topics, build_router, create_mqtt, create_state, spawn_mqtt_loop};

/// A self-contained test environment with its own MQTT topic namespace.
/// Starts the backend on an ephemeral port, creates a separate MQTT client for the "test side"
/// (simulating the outside world / device), and provides helpers for WebSocket and HTTP interaction.
/// Each instance gets a unique topic prefix so that parallel tests don't interfere with each other.
struct TestHarness {
    addr: SocketAddr,
    http: reqwest::Client,
    topics: Topics,
    /// MQTT client the *test* uses to publish/subscribe (not the backend's).
    test_mqtt: AsyncClient,
    /// Handle to the task that drains the test-side MQTT event loop.
    _test_mqtt_handle: tokio::task::JoinHandle<()>,
    /// Channel that receives MQTT publishes the test side has subscribed to.
    mqtt_rx: tokio::sync::mpsc::Receiver<(String, Vec<u8>)>,
    /// Keep backend tasks alive for the lifetime of the harness.
    _backend_mqtt_handle: tokio::task::JoinHandle<()>,
}

impl TestHarness {
    /// Spin up a fresh backend and test-side MQTT client.
    ///
    /// `test_subscriptions` — a closure receiving &Topics, returns a list of MQTT topics that the
    /// test side should subscribe to so that it can observe the what backend publishes.
    async fn new<F>(test_subscriptions: F) -> Self
    where
        F: FnOnce(&Topics) -> Vec<String>,
    {
        // Each test gets a unique client-id to avoid collisions when tests run in parallel.
        let id = uuid_short();
        let topics = Topics::new(&format!("test-{id}"));

        // Backend side:
        let (mqtt_client, eventloop) =
            create_mqtt(&format!("test-backend-{id}"), "localhost", 1883, &topics).await;
        let state = create_state(mqtt_client, topics.clone());
        let backend_mqtt_handle = spawn_mqtt_loop(eventloop, state.clone());

        let router = build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        // Test (observer) side:
        let mut opts = MqttOptions::new(format!("test-observer-{id}"), "localhost", 1883);
        opts.set_keep_alive(Duration::from_secs(30));
        let (test_mqtt, mut test_eventloop) = AsyncClient::new(opts, 50);

        let subs = test_subscriptions(&topics);
        for topic in &subs {
            test_mqtt.subscribe(topic, QoS::AtLeastOnce).await.unwrap();
        }

        // Funnel incoming publishes into a mpsc so tests can await them.
        let (mqtt_tx, mqtt_rx) = tokio::sync::mpsc::channel(64);
        let test_mqtt_handle = tokio::spawn(async move {
            loop {
                match test_eventloop.poll().await {
                    Ok(Event::Incoming(Packet::Publish(p))) => {
                        let _ = mqtt_tx.send((p.topic.clone(), p.payload.to_vec())).await;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("test-observer MQTT error: {e:?}");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            }
        });

        // Give the broker a moment to process all subscriptions.
        tokio::time::sleep(Duration::from_millis(250)).await;

        Self {
            addr,
            http: reqwest::Client::new(),
            topics,
            test_mqtt,
            _test_mqtt_handle: test_mqtt_handle,
            mqtt_rx,
            _backend_mqtt_handle: backend_mqtt_handle,
        }
    }

    /// Open a WebSocket to the backend's `/api/ws` endpoint.
    async fn connect_ws(
        &self,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let url = format!("ws://{}/api/ws", self.addr);
        let (ws, _resp) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("WebSocket connect failed");
        ws
    }

    /// Send a JSON-serialised `ClientMsg` over an open WebSocket.
    async fn ws_send<S>(ws: &mut S, msg: &ClientMsg)
    where
        S: SinkExt<tungstenite::Message> + Unpin,
        S::Error: std::fmt::Debug,
    {
        let json = serde_json::to_string(msg).unwrap();
        ws.send(tungstenite::Message::Text(json.into()))
            .await
            .expect("ws send failed");
    }

    /// Receive the next `ServerMsg` from the WebSocket, with a timeout.
    async fn ws_recv<S>(ws: &mut S, dur: Duration) -> ServerMsg
    where
        S: StreamExt<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
    {
        let msg = timeout(dur, ws.next())
            .await
            .expect("ws recv timed out")
            .expect("ws stream ended")
            .expect("ws recv error");

        match msg {
            tungstenite::Message::Text(text) => {
                serde_json::from_str::<ServerMsg>(&text).expect("failed to parse ServerMsg")
            }
            other => panic!("expected Text frame, got {other:?}"),
        }
    }

    /// Wait for an MQTT publish on the test-observer side, with a timeout.
    /// Returns `(topic, payload_bytes)`.
    async fn expect_mqtt(&mut self, dur: Duration) -> (String, Vec<u8>) {
        timeout(dur, self.mqtt_rx.recv())
            .await
            .expect("MQTT recv timed out")
            .expect("MQTT channel closed")
    }

    /// Wait for an MQTT publish on a specific topic, discarding others.
    async fn expect_mqtt_on_topic(&mut self, topic: &str, dur: Duration) -> Vec<u8> {
        let deadline = tokio::time::Instant::now() + dur;
        loop {
            let remaining = deadline - tokio::time::Instant::now();
            let (t, payload) = timeout(remaining, self.mqtt_rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for MQTT on topic '{topic}'"))
                .expect("MQTT channel closed");
            if t == topic {
                return payload;
            }
            // else: discard (e.g. a retained message on another topic)
        }
    }

    /// HTTP GET helper.
    async fn http_get(&self, path: &str) -> reqwest::Response {
        let url = format!("http://{}{}", self.addr, path);
        self.http.get(&url).send().await.expect("HTTP GET failed")
    }
}

/// Produce a short random-ish identifier
fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{:08x}-{:x}", nanos, std::process::id())
}

/// Default timeout used across tests
const T: Duration = Duration::from_secs(5);

// Pattern 1: Real-time send  (client → WS → backend → MQTT)
#[tokio::test]
async fn pattern1_realtime_send() {
    // The test side subscribes to the topic the backend publishes to.
    let mut h = TestHarness::new(|t| vec![t.send.clone()]).await;
    let mut ws = h.connect_ws().await;

    // Client sends a Publish message over WebSocket.
    let msg = ClientMsg::Publish {
        payload: "hello from test".into(),
    };
    TestHarness::ws_send(&mut ws, &msg).await;

    // The backend should forward it to MQTT on TOPIC_SEND.
    let payload = h.expect_mqtt_on_topic(&h.topics.send.clone(), T).await;
    assert_eq!(String::from_utf8_lossy(&payload), "hello from test",);
}

// Pattern 2: User-initiated poll  (MQTT → backend cache → HTTP GET)
#[tokio::test]
async fn pattern2_poll_returns_last_mqtt_message() {
    let h = TestHarness::new(|_| vec![]).await;

    // Initially: no message cached → 404.
    let resp = h.http_get("/api/last-message").await;
    assert_eq!(resp.status(), 404);

    // Publish a message on the POLL topic (simulating an external device).
    h.test_mqtt
        .publish(&h.topics.poll, QoS::AtLeastOnce, false, b"poll-value-1")
        .await
        .unwrap();

    // Give the backend's MQTT loop time to receive and cache it.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = h.http_get("/api/last-message").await;
    assert_eq!(resp.status(), 200);
    let body: LastMessage = resp.json().await.unwrap();
    assert_eq!(body.topic, h.topics.poll);
    assert_eq!(body.payload, "poll-value-1");

    // A second publish should overwrite the cached value.
    h.test_mqtt
        .publish(&h.topics.poll, QoS::AtLeastOnce, false, b"poll-value-2")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let body: LastMessage = h.http_get("/api/last-message").await.json().await.unwrap();
    assert_eq!(body.payload, "poll-value-2");
}

// Pattern 3: Real-time receive  (MQTT → backend → WS push)
#[tokio::test]
async fn pattern3_realtime_receive() {
    let h = TestHarness::new(|_| vec![]).await;
    let mut ws = h.connect_ws().await;

    // Publish on the LIVE topic from the test side.
    h.test_mqtt
        .publish(
            &h.topics.live,
            QoS::AtLeastOnce,
            false,
            b"sensor-reading-42",
        )
        .await
        .unwrap();

    // The backend should push an MqttUpdate to the WebSocket.
    let server_msg = TestHarness::ws_recv(&mut ws, T).await;
    match server_msg {
        ServerMsg::MqttUpdate { topic, payload } => {
            assert_eq!(topic, h.topics.live);
            assert_eq!(payload, "sensor-reading-42");
        }
        other => panic!("expected MqttUpdate, got {other:?}"),
    }
}

// Pattern 4: Request-response correlation  (WS → MQTT req → MQTT resp → WS)
#[tokio::test]
async fn pattern4_ping_device_roundtrip() {
    // Test side subscribes to TOPIC_PING_REQ so it can see the backend's
    // outgoing ping, then publishes a reply on TOPIC_PING_RESP.
    let mut h = TestHarness::new(|t| vec![t.ping_req.clone()]).await;
    let mut ws = h.connect_ws().await;

    let corr_id = "test-corr-001".to_string();

    // 1. Client sends PingDevice over WebSocket.
    let msg = ClientMsg::PingDevice {
        correlation_id: corr_id.clone(),
    };
    TestHarness::ws_send(&mut ws, &msg).await;

    // 2. Backend should publish a PingPayload on TOPIC_PING_REQ.
    let req_bytes = h.expect_mqtt_on_topic(&h.topics.ping_req.clone(), T).await;
    let req: PingPayload =
        serde_json::from_slice(&req_bytes).expect("failed to parse PingPayload from MQTT");
    assert_eq!(req.correlation_id, corr_id);
    assert_eq!(req.message, "ping");

    // 3. Simulate the device replying on TOPIC_PING_RESP.
    let reply = serde_json::to_vec(&PingPayload {
        correlation_id: corr_id.clone(),
        message: "pong from device".into(),
    })
    .unwrap();
    h.test_mqtt
        .publish(&h.topics.ping_resp, QoS::AtLeastOnce, false, reply)
        .await
        .unwrap();

    // 4. Backend should forward the PingResponse to the WebSocket.
    let server_msg = TestHarness::ws_recv(&mut ws, T).await;
    match server_msg {
        ServerMsg::PingResponse {
            correlation_id,
            device_reply,
        } => {
            assert_eq!(correlation_id, corr_id);
            assert_eq!(device_reply, "pong from device");
        }
        other => panic!("expected PingResponse, got {other:?}"),
    }
}

// Pattern 4 (variant): mismatched correlation ID is still delivered
#[tokio::test]
async fn pattern4_ping_unmatched_correlation_id_still_arrives() {
    // This verifies the backend is a transparent relay — it does not filter
    // by correlation_id, it forwards every PingResponse to every connected
    // WebSocket. The *client* is responsible for matching IDs.
    let h = TestHarness::new(|_| vec![]).await;
    let mut ws = h.connect_ws().await;

    // Publish a PingResponse with an ID nobody asked for.
    let reply = serde_json::to_vec(&PingPayload {
        correlation_id: "unknown-id".into(),
        message: "surprise".into(),
    })
    .unwrap();
    h.test_mqtt
        .publish(&h.topics.ping_resp, QoS::AtLeastOnce, false, reply)
        .await
        .unwrap();

    let server_msg = TestHarness::ws_recv(&mut ws, T).await;
    match server_msg {
        ServerMsg::PingResponse {
            correlation_id,
            device_reply,
        } => {
            assert_eq!(correlation_id, "unknown-id");
            assert_eq!(device_reply, "surprise");
        }
        other => panic!("expected PingResponse, got {other:?}"),
    }
}

// Edge case: multiple WebSocket clients receive the same live broadcast
#[tokio::test]
async fn broadcast_reaches_multiple_ws_clients() {
    let h = TestHarness::new(|_| vec![]).await;
    let mut ws1 = h.connect_ws().await;
    let mut ws2 = h.connect_ws().await;

    h.test_mqtt
        .publish(&h.topics.live, QoS::AtLeastOnce, false, b"broadcast-msg")
        .await
        .unwrap();

    let msg1 = TestHarness::ws_recv(&mut ws1, T).await;
    let msg2 = TestHarness::ws_recv(&mut ws2, T).await;

    for msg in [msg1, msg2] {
        match msg {
            ServerMsg::MqttUpdate { payload, .. } => {
                assert_eq!(payload, "broadcast-msg");
            }
            other => panic!("expected MqttUpdate, got {other:?}"),
        }
    }
}

// Edge case: malformed WebSocket message does not crash the backend
#[tokio::test]
async fn bad_ws_message_does_not_crash() {
    let h = TestHarness::new(|_| vec![]).await;
    let mut ws = h.connect_ws().await;

    // Send garbage JSON.
    ws.send(tungstenite::Message::Text(
        r#"{"not":"a valid ClientMsg"}"#.into(),
    ))
    .await
    .unwrap();

    // The connection should still be alive — send a valid message and check
    // the corresponding MQTT publish arrives.
    // (We need a second harness MQTT subscription for this, or we can just
    // verify the WS stays open by checking we can still receive.)
    //
    // Simplest check: publish something on LIVE and see if the WS delivers.
    h.test_mqtt
        .publish(&h.topics.live, QoS::AtLeastOnce, false, b"still-alive")
        .await
        .unwrap();

    let msg = TestHarness::ws_recv(&mut ws, T).await;
    match msg {
        ServerMsg::MqttUpdate { payload, .. } => {
            assert_eq!(payload, "still-alive");
        }
        other => panic!("expected MqttUpdate, got {other:?}"),
    }
}
