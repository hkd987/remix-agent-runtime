use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures_core::Stream;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::events::EventBus;

#[derive(Clone)]
struct AppState {
    event_bus: Arc<EventBus>,
}

async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_bus.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => {
            let event_type = event.event_type().to_string();
            match serde_json::to_string(&event) {
                Ok(json) => Some(Ok(Event::default().event(event_type).data(json))),
                Err(_) => None,
            }
        }
        Err(_) => None, // lagged subscriber, skip
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

async fn health_handler() -> &'static str {
    "ok"
}

/// Build the axum Router for the SSE server.
pub fn build_router(event_bus: Arc<EventBus>) -> Router {
    let state = AppState { event_bus };
    Router::new()
        .route("/events", get(sse_handler))
        .route("/health", get(health_handler))
        .with_state(state)
}

/// Start the SSE server on the given port. This function runs until the server shuts down.
pub async fn start_sse_server(
    event_bus: Arc<EventBus>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = build_router(event_bus);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!(port = port, "SSE server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::events::AgentEvent;

    #[tokio::test]
    async fn test_health_endpoint_returns_ok() {
        let bus = Arc::new(EventBus::new(16));
        let app = build_router(bus);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn test_sse_endpoint_returns_event_stream_content_type() {
        let bus = Arc::new(EventBus::new(16));
        let app = build_router(bus);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/events"))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            content_type.contains("text/event-stream"),
            "Expected text/event-stream, got: {content_type}"
        );
    }

    #[tokio::test]
    async fn test_sse_streams_events_to_client() {
        let bus = Arc::new(EventBus::new(16));
        let app = build_router(bus.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Connect to SSE endpoint
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/events"))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        // Send an event after a small delay to ensure the handler has subscribed
        let bus_clone = bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            bus_clone.send(AgentEvent::IterationStarted { iteration: 42 });
        });

        // Read the first chunk from the stream
        let mut stream = resp.bytes_stream();
        let chunk = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            StreamExt::next(&mut stream),
        )
        .await
        .expect("Timed out waiting for SSE event")
        .expect("Stream ended unexpectedly")
        .expect("Error reading stream");

        let text = String::from_utf8_lossy(&chunk);
        assert!(
            text.contains("iteration_started"),
            "Expected iteration_started event, got: {text}"
        );
        assert!(
            text.contains("42"),
            "Expected iteration 42 in event, got: {text}"
        );
    }

    #[test]
    fn test_build_router_creates_valid_router() {
        let bus = Arc::new(EventBus::new(16));
        let _router = build_router(bus);
    }
}
