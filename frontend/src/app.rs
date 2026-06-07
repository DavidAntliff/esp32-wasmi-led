use eframe::egui;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{Message as WsMessage, futures::WebSocket};
use uuid::Uuid;
use web_common::{ClientMsg, LastMessage, ServerMsg};

// Channel messages internal to the frontend
enum ToBackend {
    Send(ClientMsg),
}

// From the backend:
enum IncomingEvent {
    Connected,
    Disconnected,
    Msg(ServerMsg),
}

pub struct PrototypeApp {
    // Outbound channel: UI thread → WebSocket send task
    ws_tx: mpsc::UnboundedSender<ToBackend>,

    // Inbound: messages received from WebSocket
    incoming_rx: mpsc::UnboundedReceiver<IncomingEvent>,

    // UI state
    connected: bool,
    send_payload: String,
    last_fetched: Option<LastMessage>,
    live_messages: Vec<String>,
    ping_responses: Vec<String>,
    fetch_error: Option<String>,

    // Internal Fetch results channel
    fetch_tx: mpsc::UnboundedSender<Result<LastMessage, String>>,
    fetch_rx: mpsc::UnboundedReceiver<Result<LastMessage, String>>,
}

impl PrototypeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[allow(unused_variables)]
        let (ws_tx, _ws_rx) = mpsc::unbounded::<ToBackend>();
        #[allow(unused_variables)]
        let (_incoming_tx, incoming_rx) = mpsc::unbounded::<IncomingEvent>();

        #[cfg(target_arch = "wasm32")]
        {
            let ctx = cc.egui_ctx.clone();

            // Rebind with actual connected channels
            let (ws_tx, ws_rx) = mpsc::unbounded::<ToBackend>();
            let (incoming_tx, incoming_rx) = mpsc::unbounded::<IncomingEvent>();

            // Spawn the WebSocket manager task
            wasm_bindgen_futures::spawn_local(ws_task(ws_rx, incoming_tx, ctx));

            let (fetch_tx, fetch_rx) = mpsc::unbounded();

            Self {
                ws_tx,
                incoming_rx,
                connected: false,
                send_payload: "Hello from egui!".into(),
                last_fetched: None,
                live_messages: Vec::new(),
                ping_responses: Vec::new(),
                fetch_error: None,
                fetch_tx,
                fetch_rx,
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let (fetch_tx, fetch_rx) = mpsc::unbounded();

            Self {
                ws_tx,
                incoming_rx,
                connected: false,
                send_payload: "Hello from egui!".into(),
                last_fetched: None,
                live_messages: Vec::new(),
                ping_responses: Vec::new(),
                fetch_error: None,
                fetch_tx,
                fetch_rx,
            }
        }
    }
}

impl eframe::App for PrototypeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain messages from websocket task:
        while let Ok(event) = self.incoming_rx.try_recv() {
            match event {
                IncomingEvent::Connected => self.connected = true,
                IncomingEvent::Disconnected => self.connected = false,
                IncomingEvent::Msg(msg) => {
                    match msg {
                        ServerMsg::MqttUpdate { topic, payload } => {
                            self.live_messages.push(format!("[{topic}] {payload}"));
                            // Keep last 50
                            if self.live_messages.len() > 50 {
                                self.live_messages.remove(0);
                            }
                        }
                        ServerMsg::PingResponse {
                            correlation_id,
                            device_reply,
                        } => {
                            self.ping_responses
                                .push(format!("[{correlation_id}] {device_reply}"));
                        }
                    }
                }
            }
        }

        // Drain fetch results, if any:
        while let Ok(result) = self.fetch_rx.try_recv() {
            match result {
                Ok(msg) => {
                    self.last_fetched = Some(msg);
                    self.fetch_error = None;
                }
                Err(e) => {
                    self.fetch_error = Some(e);
                }
            }
        }

        // Update UI
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("MQTT Prototype");

            // Connection indicator
            ui.horizontal(|ui| {
                let (color, text) = if self.connected {
                    (egui::Color32::GREEN, "● Connected")
                } else {
                    (egui::Color32::RED, "● Disconnected")
                };
                ui.colored_label(color, text);
            });

            ui.separator();

            // Real-time send to backend:
            ui.group(|ui| {
                ui.label("Real-time Send (WebSocket → MQTT)");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.send_payload);
                    if ui.button("Send").clicked() {
                        let msg = ClientMsg::Publish {
                            payload: self.send_payload.clone(),
                        };
                        let _ = self.ws_tx.unbounded_send(ToBackend::Send(msg));
                    }
                });
            });

            ui.separator();

            // User-initiated poll from backend:
            ui.group(|ui| {
                ui.label("User-Initiated Poll (HTTP GET)");
                if ui.button("Fetch").clicked() {
                    let ctx = ctx.clone();
                    #[cfg(target_arch = "wasm32")]
                    fetch_last_message(ctx, self.fetch_tx.clone());
                }
                if let Some(ref msg) = self.last_fetched {
                    ui.label(format!("Topic: {}", msg.topic));
                    ui.label(format!("Payload: {}", msg.payload));
                    ui.label(format!("Timestamp: {}", msg.timestamp_ms));
                }
                if let Some(ref err) = self.fetch_error {
                    ui.colored_label(egui::Color32::RED, err);
                }
            });

            ui.separator();

            // Real-time receive from backend:
            ui.group(|ui| {
                ui.label("Real-time Receive (MQTT → WebSocket)");
                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for msg in &self.live_messages {
                            ui.label(msg);
                        }
                    });
            });

            ui.separator();

            // Request-response correlation:
            ui.group(|ui| {
                ui.label("Ping Device (Request-Response over MQTT)");
                if ui.button("Ping Device").clicked() {
                    let correlation_id = Uuid::new_v4().to_string();
                    let msg = ClientMsg::PingDevice { correlation_id };
                    let _ = self.ws_tx.unbounded_send(ToBackend::Send(msg));
                }
                for resp in &self.ping_responses {
                    ui.label(resp);
                }
            });
        });
    }
}

// WebSocket management task
// (Runs as a spawned future in the browser)
#[cfg(target_arch = "wasm32")]
async fn ws_task(
    mut commands: mpsc::UnboundedReceiver<ToBackend>,
    mut incoming: mpsc::UnboundedSender<IncomingEvent>,
    ctx: egui::Context,
) {
    // Derive the WebSocket URL from the current page origin
    let window = web_sys::window().unwrap();
    let origin = window.location().origin().unwrap();
    let ws_url = format!("{}/api/ws", origin.replace("http", "ws"));

    // TODO: reconnection logic
    let ws = match WebSocket::open(&ws_url) {
        Ok(ws) => ws,
        Err(e) => {
            tracing::error!("WebSocket open failed: {e:?}");
            return;
        }
    };

    let (mut ws_write, mut ws_read) = ws.split();

    let _ = incoming.send(IncomingEvent::Connected).await;
    ctx.request_repaint();

    // Forward outbound commands to the WebSocket
    let write_task = async {
        while let Some(ToBackend::Send(msg)) = commands.next().await {
            let json = serde_json::to_string(&msg).unwrap();
            if ws_write.send(WsMessage::Text(json)).await.is_err() {
                break;
            }
        }
    };

    // Forward inbound WebSocket messages to the UI
    let read_task = async {
        while let Some(Ok(msg)) = ws_read.next().await {
            if let WsMessage::Text(text) = msg
                && let Ok(server_msg) = serde_json::from_str::<ServerMsg>(&text)
            {
                let _ = incoming.send(IncomingEvent::Msg(server_msg)).await;
                ctx.request_repaint(); // Wake up egui
            }
        }
    };

    // Run both tasks concurrently; if either ends, we're disconnected
    futures::future::select(Box::pin(write_task), Box::pin(read_task)).await;

    let _ = incoming.send(IncomingEvent::Disconnected).await;
    ctx.request_repaint();
}

// HTTP fetch task for the poll button - send result through the fetch channel
#[cfg(target_arch = "wasm32")]
fn fetch_last_message(
    ctx: egui::Context,
    mut tx: mpsc::UnboundedSender<Result<LastMessage, String>>,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let resp = gloo_net::http::Request::get("/api/last-message")
            .send()
            .await;
        let msg = match resp {
            Ok(r) if r.ok() => r.json::<LastMessage>().await.map_err(|e| e.to_string()),
            Ok(r) => Err(format!("HTTP {}", r.status())),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(msg).await;
        ctx.request_repaint();
    });
}
