use backend::{Topics, build_app, create_mqtt, create_state, spawn_mqtt_loop};
use tracing::info;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let topics = Topics::default();
    let (mqtt_client, eventloop) =
        create_mqtt("egui-axum-mqtt-backend", "localhost", 1883, &topics).await;
    let state = create_state(mqtt_client, topics);
    let _mqtt_handle = spawn_mqtt_loop(eventloop, state.clone());

    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    info!("Listening on http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}
