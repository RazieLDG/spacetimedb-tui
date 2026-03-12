//! WebSocket client for real-time SpacetimeDB subscriptions and log streaming.
//!
//! [`WsClient`] connects to the SpacetimeDB WebSocket endpoint and forwards
//! decoded messages over a [`tokio::sync::mpsc`] channel so that the TUI
//! event loop can consume them without blocking.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{handshake::client::Request, Message},
};
use tracing::{debug, error, info, warn};
use url::Url;

use super::types::{LogEntry, WsServerMessage};

// ---------------------------------------------------------------------------
// Public message enum delivered to the TUI
// ---------------------------------------------------------------------------

/// Messages produced by the WebSocket background task and sent over the mpsc
/// channel to the TUI event loop.
#[derive(Debug, Clone)]
pub enum WsEvent {
    /// A decoded server message (subscription update, identity token, …).
    ServerMessage(WsServerMessage),
    /// A log line streamed from the server (log-follow mode).
    LogLine(LogEntry),
    /// Raw text frame that could not be decoded as a known message type.
    /// The inner `String` is preserved for diagnostic logging.
    RawText(String),
    /// The WebSocket connection was successfully established.
    Connected,
    /// The connection was closed (gracefully or by error).
    Disconnected { reason: String },
    /// A non-fatal error occurred (e.g. a single bad frame).
    Error(String),
}

// ---------------------------------------------------------------------------
// Client-to-server message types
// ---------------------------------------------------------------------------

/// A subscription request sent to the server.
#[derive(Debug, Serialize)]
struct SubscribeMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    query_strings: Vec<String>,
    request_id: u32,
}

impl SubscribeMessage {
    fn new(queries: Vec<String>, request_id: u32) -> Self {
        Self {
            msg_type: "Subscribe",
            query_strings: queries,
            request_id,
        }
    }
}

/// A reducer call request (reserved for future use).
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct CallReducerMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub reducer: String,
    pub args: serde_json::Value,
    pub request_id: u32,
}

// ---------------------------------------------------------------------------
// WsClient
// ---------------------------------------------------------------------------

/// Configuration for a WebSocket connection.
#[derive(Debug, Clone)]
pub struct WsConfig {
    /// WebSocket base URL, e.g. `ws://localhost:3000`.
    pub base_url: String,
    /// Database / module name.
    pub database: String,
    /// Optional bearer token for authentication.
    pub auth_token: Option<String>,
    /// Capacity of the mpsc channel buffer.
    pub channel_capacity: usize,
}

impl WsConfig {
    /// Build the full WebSocket URL for a subscription connection.
    pub fn subscription_url(&self) -> Result<Url> {
        let raw = format!("{}/v1/database/{}/subscribe", self.base_url, self.database);
        Url::parse(&raw).with_context(|| format!("Invalid WebSocket URL: {raw}"))
    }

    /// Build the full WebSocket URL for a log-follow connection.
    ///
    /// Used by [`spawn_log_follow`] for streaming live log output.
    #[allow(dead_code)]
    pub fn log_follow_url(&self) -> Result<Url> {
        let raw = format!(
            "{}/v1/database/{}/logs?follow=true&num_lines=100",
            self.base_url, self.database
        );
        Url::parse(&raw).with_context(|| format!("Invalid log follow URL: {raw}"))
    }
}

/// A handle to a running WebSocket background task.
///
/// Dropping this handle does **not** automatically close the connection;
/// call [`WsHandle::close`] explicitly or drop the underlying task.
#[derive(Debug)]
pub struct WsHandle {
    /// Send commands to the background task.
    cmd_tx: mpsc::Sender<WsCommand>,
    /// Receive events from the background task.
    pub event_rx: mpsc::Receiver<WsEvent>,
}

impl WsHandle {
    /// Send a subscription request to the server.
    pub async fn subscribe(&self, queries: Vec<String>, request_id: u32) -> Result<()> {
        self.cmd_tx
            .send(WsCommand::Subscribe { queries, request_id })
            .await
            .context("WebSocket task has shut down")
    }

    /// Request a graceful shutdown of the background task.
    pub async fn close(&self) {
        let _ = self.cmd_tx.send(WsCommand::Close).await;
    }
}

/// Commands sent from the TUI to the WebSocket background task.
#[derive(Debug)]
enum WsCommand {
    Subscribe { queries: Vec<String>, request_id: u32 },
    Close,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Spawn a WebSocket subscription task for `config.database`.
///
/// Returns a [`WsHandle`] through which the caller can send subscription
/// requests and receive [`WsEvent`]s.
pub fn spawn_subscription(config: WsConfig) -> Result<WsHandle> {
    let url = config.subscription_url()?;
    let (cmd_tx, cmd_rx) = mpsc::channel::<WsCommand>(32);
    let (event_tx, event_rx) = mpsc::channel::<WsEvent>(config.channel_capacity);

    tokio::spawn(subscription_task(url, config.auth_token.clone(), cmd_rx, event_tx));

    Ok(WsHandle { cmd_tx, event_rx })
}

/// Spawn a WebSocket log-follow task for `config.database`.
///
/// Log lines are forwarded as [`WsEvent::LogLine`] events.
/// Available for future integration with the Logs tab live streaming.
#[allow(dead_code)]
pub fn spawn_log_follow(config: WsConfig) -> Result<WsHandle> {
    let url = config.log_follow_url()?;
    let (cmd_tx, cmd_rx) = mpsc::channel::<WsCommand>(32);
    let (event_tx, event_rx) = mpsc::channel::<WsEvent>(config.channel_capacity);

    tokio::spawn(log_follow_task(url, config.auth_token.clone(), cmd_rx, event_tx));

    Ok(WsHandle { cmd_tx, event_rx })
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

/// Build an HTTP upgrade request with optional auth header.
fn build_ws_request(url: Url, auth_token: Option<&str>) -> Result<Request> {
    let mut builder = Request::builder().uri(url.as_str());

    if let Some(token) = auth_token {
        let value = format!("Bearer {}", token);
        builder = builder.header("Authorization", value);
    }

    // Request JSON encoding so frames can be decoded with serde_json.
    // SpacetimeDB 2.0 supports both "v1.bsatn.spacetimedb" (binary) and
    // "v1.json.spacetimedb" (JSON).  We use JSON to avoid a BSATN decoder.
    builder = builder.header("Sec-WebSocket-Protocol", "v1.json.spacetimedb");

    builder
        .body(())
        .context("Failed to build WebSocket upgrade request")
}

/// Main loop for a subscription WebSocket connection.
async fn subscription_task(
    url: Url,
    auth_token: Option<String>,
    mut cmd_rx: mpsc::Receiver<WsCommand>,
    event_tx: mpsc::Sender<WsEvent>,
) {
    info!("Connecting to subscription WebSocket: {}", url);

    let request = match build_ws_request(url.clone(), auth_token.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            let _ = event_tx
                .send(WsEvent::Disconnected {
                    reason: format!("Request build error: {e}"),
                })
                .await;
            return;
        }
    };

    let (ws_stream, _) = match connect_async(request).await {
        Ok(pair) => pair,
        Err(e) => {
            error!("WebSocket connect failed: {e}");
            let _ = event_tx
                .send(WsEvent::Disconnected {
                    reason: format!("Connect error: {e}"),
                })
                .await;
            return;
        }
    };

    info!("WebSocket connected: {}", url);
    let _ = event_tx.send(WsEvent::Connected).await;

    let (mut sink, mut stream) = ws_stream.split();

    loop {
        tokio::select! {
            // Inbound frames from the server.
            msg = stream.next() => {
                match msg {
                    Some(Ok(frame)) => {
                        if let Some(event) = decode_subscription_frame(frame) {
                            if event_tx.send(event).await.is_err() {
                                debug!("Event receiver dropped; closing WebSocket task");
                                break;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket frame error: {e}");
                        let _ = event_tx.send(WsEvent::Error(e.to_string())).await;
                        // Attempt to continue; a fatal error will manifest as
                        // a `None` on the next iteration.
                    }
                    None => {
                        info!("WebSocket stream closed by server");
                        let _ = event_tx
                            .send(WsEvent::Disconnected {
                                reason: "Server closed the connection".to_string(),
                            })
                            .await;
                        break;
                    }
                }
            }

            // Commands from the TUI.
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(WsCommand::Subscribe { queries, request_id }) => {
                        let msg = SubscribeMessage::new(queries, request_id);
                        let json = match serde_json::to_string(&msg) {
                            Ok(j) => j,
                            Err(e) => {
                                warn!("Failed to serialise Subscribe message: {e}");
                                continue;
                            }
                        };
                        if let Err(e) = sink.send(Message::Text(json.into())).await {
                            error!("Failed to send Subscribe frame: {e}");
                            let _ = event_tx
                                .send(WsEvent::Disconnected {
                                    reason: format!("Send error: {e}"),
                                })
                                .await;
                            break;
                        }
                    }
                    Some(WsCommand::Close) | None => {
                        info!("WebSocket task received close command");
                        let _ = sink.send(Message::Close(None)).await;
                        let _ = event_tx
                            .send(WsEvent::Disconnected {
                                reason: "Client requested close".to_string(),
                            })
                            .await;
                        break;
                    }
                }
            }
        }
    }
}

/// Main loop for a log-follow WebSocket connection.
#[allow(dead_code)]
async fn log_follow_task(
    url: Url,
    auth_token: Option<String>,
    mut cmd_rx: mpsc::Receiver<WsCommand>,
    event_tx: mpsc::Sender<WsEvent>,
) {
    info!("Connecting to log-follow WebSocket: {}", url);

    let request = match build_ws_request(url.clone(), auth_token.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            let _ = event_tx
                .send(WsEvent::Disconnected {
                    reason: format!("Request build error: {e}"),
                })
                .await;
            return;
        }
    };

    let (ws_stream, _) = match connect_async(request).await {
        Ok(pair) => pair,
        Err(e) => {
            error!("Log WebSocket connect failed: {e}");
            let _ = event_tx
                .send(WsEvent::Disconnected {
                    reason: format!("Connect error: {e}"),
                })
                .await;
            return;
        }
    };

    info!("Log WebSocket connected: {}", url);
    let _ = event_tx.send(WsEvent::Connected).await;

    let (mut sink, mut stream) = ws_stream.split();

    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(frame)) => {
                        match frame {
                            Message::Text(text) => {
                                let text_str = text.as_str();
                                match serde_json::from_str::<LogEntry>(text_str) {
                                    Ok(entry) => {
                                        if event_tx.send(WsEvent::LogLine(entry)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => {
                                        // Not a structured log entry — forward as raw text.
                                        if event_tx
                                            .send(WsEvent::RawText(text_str.to_owned()))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                            Message::Close(_) => {
                                let _ = event_tx
                                    .send(WsEvent::Disconnected {
                                        reason: "Server closed log stream".to_string(),
                                    })
                                    .await;
                                break;
                            }
                            Message::Ping(data) => {
                                // Respond to pings to keep the connection alive.
                                let _ = sink.send(Message::Pong(data)).await;
                            }
                            _ => {}
                        }
                    }
                    Some(Err(e)) => {
                        warn!("Log WebSocket frame error: {e}");
                        let _ = event_tx.send(WsEvent::Error(e.to_string())).await;
                    }
                    None => {
                        let _ = event_tx
                            .send(WsEvent::Disconnected {
                                reason: "Log stream ended".to_string(),
                            })
                            .await;
                        break;
                    }
                }
            }

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(WsCommand::Close) | None => {
                        let _ = sink.send(Message::Close(None)).await;
                        let _ = event_tx
                            .send(WsEvent::Disconnected {
                                reason: "Client requested close".to_string(),
                            })
                            .await;
                        break;
                    }
                    _ => {} // Log-follow doesn't handle Subscribe commands.
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Frame decoders
// ---------------------------------------------------------------------------

/// Decode an inbound WebSocket frame from the subscription endpoint.
fn decode_subscription_frame(frame: Message) -> Option<WsEvent> {
    match frame {
        Message::Text(text) => {
            let text_str = text.as_str();
            match serde_json::from_str::<WsServerMessage>(text_str) {
                Ok(msg) => Some(WsEvent::ServerMessage(msg)),
                Err(e) => {
                    debug!("Could not decode server message: {e} — raw: {}", text_str);
                    Some(WsEvent::RawText(text_str.to_owned()))
                }
            }
        }
        Message::Binary(bytes) => {
            // BSATN binary frames — attempt UTF-8 fallback for diagnostics.
            match std::str::from_utf8(&bytes) {
                Ok(s) => Some(WsEvent::RawText(s.to_owned())),
                Err(_) => {
                    debug!("Received {} binary bytes (BSATN)", bytes.len());
                    None
                }
            }
        }
        Message::Ping(_) | Message::Pong(_) => None,
        Message::Close(frame) => {
            let reason = frame
                .as_ref()
                .map(|f| f.reason.to_string())
                .unwrap_or_else(|| "no reason".to_string());
            Some(WsEvent::Disconnected { reason })
        }
        Message::Frame(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscription_url() {
        let cfg = WsConfig {
            base_url: "ws://localhost:3000".to_string(),
            database: "mydb".to_string(),
            auth_token: None,
            channel_capacity: 64,
        };
        let url = cfg.subscription_url().unwrap();
        assert_eq!(url.as_str(), "ws://localhost:3000/v1/database/mydb/subscribe");
    }

    #[test]
    fn test_log_follow_url() {
        let cfg = WsConfig {
            base_url: "ws://localhost:3000".to_string(),
            database: "mydb".to_string(),
            auth_token: None,
            channel_capacity: 64,
        };
        let url = cfg.log_follow_url().unwrap();
        assert!(url.as_str().contains("/v1/database/mydb/logs"));
        assert!(url.as_str().contains("follow=true"));
    }

    #[test]
    fn test_build_ws_request_no_auth() {
        let url = Url::parse("ws://localhost:3000/v1/database/test/subscribe").unwrap();
        let req = build_ws_request(url, None).unwrap();
        assert!(req.headers().get("Authorization").is_none());
    }

    #[test]
    fn test_build_ws_request_with_auth() {
        let url = Url::parse("ws://localhost:3000/v1/database/test/subscribe").unwrap();
        let req = build_ws_request(url, Some("mytoken")).unwrap();
        let auth = req.headers().get("Authorization").unwrap();
        assert_eq!(auth, "Bearer mytoken");
    }
}
