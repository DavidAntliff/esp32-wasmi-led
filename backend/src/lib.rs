use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tower_http::services::ServeDir;
use tracing::{error, info, warn};
use web_common::{ClientMsg, LastMessage, ServerMsg};

// Default MQTT topic prefix (production):
pub const DEFAULT_PREFIX: &str = "esp32-wasmi-led";

/// MQTT topic configuration, to support concurrent testing with unique prefixes
#[derive(Debug, Clone)]
pub struct Topics {
    pub send: String,
    pub poll: String,
    pub live: String,
    pub ping_req: String,
    pub ping_resp: String,
}

impl Topics {
    pub fn new(prefix: &str) -> Self {
        Self {
            send: format!("{prefix}/send"),
            poll: format!("{prefix}/poll"),
            live: format!("{prefix}/live"),
            ping_req: format!("{prefix}/ping/request"),
            ping_resp: format!("{prefix}/ping/response"),
        }
    }
}

impl Default for Topics {
    fn default() -> Self {
        Self::new(DEFAULT_PREFIX)
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub mqtt_client: AsyncClient,
    pub last_poll_msg: Arc<RwLock<Option<LastMessage>>>,
    /// Broadcast channel: backend + all WebSocket clients
    pub tx: broadcast::Sender<ServerMsg>,
    pub topics: Topics,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PingPayload {
    pub correlation_id: String,
    pub message: String,
}

/// Create MQTT client and event loop, and subscribe to relevant topics
pub async fn create_mqtt(
    client_id: &str,
    host: &str,
    port: u16,
    topics: &Topics,
) -> (AsyncClient, EventLoop) {
    let mut opts = MqttOptions::new(client_id, host, port);
    opts.set_keep_alive(std::time::Duration::from_secs(30));

    let (client, eventloop) = AsyncClient::new(opts, 50);

    client
        .subscribe(&topics.poll, QoS::AtLeastOnce)
        .await
        .unwrap();
    client
        .subscribe(&topics.live, QoS::AtLeastOnce)
        .await
        .unwrap();
    client
        .subscribe(&topics.ping_resp, QoS::AtLeastOnce)
        .await
        .unwrap();

    (client, eventloop)
}

/// Build the shared application state
pub fn create_state(mqtt_client: AsyncClient, topics: Topics) -> AppState {
    let (tx, _rx) = broadcast::channel::<ServerMsg>(100);

    AppState {
        mqtt_client,
        last_poll_msg: Arc::new(RwLock::new(None)),
        tx,
        topics,
    }
}

/// Spawn the MQTT + AppState bridging loop, returning the JoinHandle
pub fn spawn_mqtt_loop(mut eventloop: EventLoop, state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let topic = publish.topic.clone();
                    let payload = String::from_utf8_lossy(&publish.payload).to_string();
                    info!("MQTT recv: {topic} -> {payload}");

                    match topic.as_str() {
                        t if t == state.topics.poll => {
                            let msg = LastMessage {
                                topic: topic.clone(),
                                payload: payload.clone(),
                                timestamp_ms: now_ms(),
                            };
                            *state.last_poll_msg.write().unwrap() = Some(msg);
                        }
                        t if t == state.topics.live => {
                            match state.tx.send(ServerMsg::MqttUpdate { topic, payload }) {
                                Ok(_) => {}
                                Err(e) => {
                                    error!("MQTT send error: {e}")
                                }
                            }
                        }
                        t if t == state.topics.ping_resp => {
                            if let Ok(resp) = serde_json::from_str::<PingPayload>(&payload) {
                                match state.tx.send(ServerMsg::PingResponse {
                                    correlation_id: resp.correlation_id,
                                    device_reply: resp.message,
                                }) {
                                    Ok(_) => {}
                                    Err(e) => {
                                        error!("MQTT send error: {e}")
                                    }
                                }
                            } else {
                                warn!("Invalid JSON: {payload:?}");
                            }
                        }
                        _ => {
                            warn!("Received message on unexpected topic: {topic}");
                        }
                    }
                }
                Ok(_) => {} // connack, suback, etc.
                Err(e) => {
                    warn!("MQTT error: {e:?}");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    })
}

/// Build the Axum router (without fallback, for testing)
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/ws", get(ws_handler))
        .route("/api/last-message", get(get_last_message))
        .with_state(state)
}

/// Build the full router including static file serving
pub fn build_app(state: AppState) -> Router {
    // Serve the frontend Wasm app from ./dist (trunk output)
    build_router(state).fallback_service(ServeDir::new("dist"))
}

// HTTP handler: user-initiated poll
async fn get_last_message(State(state): State<AppState>) -> impl IntoResponse {
    let msg = state.last_poll_msg.read().unwrap().clone();
    match msg {
        Some(m) => Json(m).into_response(),
        None => (axum::http::StatusCode::NOT_FOUND, "No messages yet").into_response(),
    }
}

// WebSocket handler
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.tx.subscribe();

    loop {
        tokio::select! {
            // Messages from the frontend:
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(ClientMsg::Publish { payload }) => {
                                info!("Publishing to MQTT: {payload}");
                                let _ = state.mqtt_client
                                    .publish(&state.topics.send, QoS::AtLeastOnce, false, payload.as_bytes())
                                    .await;
                            }
                            Ok(ClientMsg::PingDevice { correlation_id }) => {
                                info!("Pinging Device: {correlation_id}");
                                let ping = serde_json::to_string(&PingPayload {
                                    correlation_id,
                                    message: "ping".into(),
                                }).unwrap();
                                let _ = state.mqtt_client
                                    .publish(&state.topics.ping_req, QoS::AtLeastOnce, false, ping.as_bytes())
                                    .await;
                            }
                            Err(e) => warn!("Bad client message: {e}"),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }

            // Messages from MQTT → push to frontend:
            Ok(server_msg) = rx.recv() => {
                let json = serde_json::to_string(&server_msg).unwrap();
                if socket.send(Message::Text(json.into())).await.is_err() {
                    break; // client disconnected
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
