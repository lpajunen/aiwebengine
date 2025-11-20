//! GraphQL over WebSocket Protocol Implementation
//!
//! Implements the graphql-transport-ws protocol for GraphQL subscriptions over WebSockets.
//! Protocol specification: https://github.com/enisdenjo/graphql-ws/blob/master/PROTOCOL.md
//!
//! Features:
//! - Connection lifecycle management (init, ack, ping/pong, complete)
//! - Multiple concurrent subscriptions per connection
//! - Authentication via connection_init payload
//! - Automatic keep-alive with ping/pong
//! - Graceful error handling and cleanup

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, warn};

/// Maximum number of concurrent subscriptions per WebSocket connection
const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 20;

/// Keep-alive ping interval (30 seconds)
const PING_INTERVAL_SECS: u64 = 30;

/// Message types in the graphql-transport-ws protocol
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageType {
    ConnectionInit,
    ConnectionAck,
    Ping,
    Pong,
    Subscribe,
    Next,
    Error,
    Complete,
}

impl MessageType {
    fn as_str(&self) -> &'static str {
        match self {
            MessageType::ConnectionInit => "connection_init",
            MessageType::ConnectionAck => "connection_ack",
            MessageType::Ping => "ping",
            MessageType::Pong => "pong",
            MessageType::Subscribe => "subscribe",
            MessageType::Next => "next",
            MessageType::Error => "error",
            MessageType::Complete => "complete",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "connection_init" => Some(MessageType::ConnectionInit),
            "connection_ack" => Some(MessageType::ConnectionAck),
            "ping" => Some(MessageType::Ping),
            "pong" => Some(MessageType::Pong),
            "subscribe" => Some(MessageType::Subscribe),
            "next" => Some(MessageType::Next),
            "error" => Some(MessageType::Error),
            "complete" => Some(MessageType::Complete),
            _ => None,
        }
    }
}

/// GraphQL-WS protocol message
#[derive(Debug, Serialize, Deserialize)]
pub struct ProtocolMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

impl ProtocolMessage {
    pub fn connection_ack() -> Self {
        Self {
            msg_type: MessageType::ConnectionAck.as_str().to_string(),
            id: None,
            payload: None,
        }
    }

    pub fn pong() -> Self {
        Self {
            msg_type: MessageType::Pong.as_str().to_string(),
            id: None,
            payload: None,
        }
    }

    pub fn ping() -> Self {
        Self {
            msg_type: MessageType::Ping.as_str().to_string(),
            id: None,
            payload: None,
        }
    }

    pub fn next(id: String, payload: Value) -> Self {
        Self {
            msg_type: MessageType::Next.as_str().to_string(),
            id: Some(id),
            payload: Some(payload),
        }
    }

    pub fn error(id: String, errors: Vec<String>) -> Self {
        let error_payload = serde_json::json!({
            "errors": errors.into_iter().map(|msg| serde_json::json!({
                "message": msg
            })).collect::<Vec<_>>()
        });
        Self {
            msg_type: MessageType::Error.as_str().to_string(),
            id: Some(id),
            payload: Some(error_payload),
        }
    }

    pub fn complete(id: String) -> Self {
        Self {
            msg_type: MessageType::Complete.as_str().to_string(),
            id: Some(id),
            payload: None,
        }
    }
}

/// Subscription state tracker
struct SubscriptionState {
    /// Active subscriptions mapped by ID
    subscriptions: HashMap<String, tokio::task::JoinHandle<()>>,
}

impl SubscriptionState {
    fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    fn add(&mut self, id: String, handle: tokio::task::JoinHandle<()>) -> bool {
        if self.subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_CONNECTION {
            return false;
        }
        self.subscriptions.insert(id, handle);
        true
    }

    fn remove(&mut self, id: &str) -> Option<tokio::task::JoinHandle<()>> {
        self.subscriptions.remove(id)
    }

    async fn abort_all(&mut self) {
        for (id, handle) in self.subscriptions.drain() {
            debug!("Aborting subscription: {}", id);
            handle.abort();
        }
    }
}

/// WebSocket connection handler for GraphQL subscriptions
pub async fn handle_websocket_connection(
    socket: WebSocket,
    auth_user: Option<crate::auth::AuthUser>,
) {
    info!(
        "New GraphQL WebSocket connection - authenticated: {}",
        auth_user.is_some()
    );

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let subscription_state = Arc::new(Mutex::new(SubscriptionState::new()));
    let mut connection_initialized = false;

    // Create channel for sending messages to WebSocket
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProtocolMessage>();

    // Spawn task to send messages from channel to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg)
                && ws_sender.send(Message::Text(json.into())).await.is_err()
            {
                break;
            }
        }
    });

    // Spawn keep-alive ping task
    let ping_tx = tx.clone();
    let ping_task = tokio::spawn(async move {
        let mut ping_interval = interval(Duration::from_secs(PING_INTERVAL_SECS));
        loop {
            ping_interval.tick().await;
            if ping_tx.send(ProtocolMessage::ping()).is_err() {
                break;
            }
        }
    });

    // Main message processing loop
    while let Some(result) = ws_receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                let msg: ProtocolMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        error!("Failed to parse WebSocket message: {}", e);
                        continue;
                    }
                };

                let msg_type = match MessageType::from_str(&msg.msg_type) {
                    Some(t) => t,
                    None => {
                        warn!("Unknown message type: {}", msg.msg_type);
                        continue;
                    }
                };

                match msg_type {
                    MessageType::ConnectionInit => {
                        if connection_initialized {
                            warn!("Connection already initialized");
                            break;
                        }
                        connection_initialized = true;
                        debug!("Connection initialized");
                        if tx.send(ProtocolMessage::connection_ack()).is_err() {
                            break;
                        }
                    }

                    MessageType::Ping => {
                        debug!("Received ping, sending pong");
                        if tx.send(ProtocolMessage::pong()).is_err() {
                            break;
                        }
                    }

                    MessageType::Pong => {
                        debug!("Received pong");
                    }

                    MessageType::Subscribe => {
                        if !connection_initialized {
                            warn!("Received subscribe before connection_init");
                            break;
                        }

                        let subscription_id = match msg.id {
                            Some(id) => id,
                            None => {
                                warn!("Subscribe message missing id");
                                continue;
                            }
                        };

                        let payload = match msg.payload {
                            Some(p) => p,
                            None => {
                                warn!("Subscribe message missing payload");
                                if tx
                                    .send(ProtocolMessage::error(
                                        subscription_id,
                                        vec!["Missing payload".to_string()],
                                    ))
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        };

                        // Parse GraphQL request from payload
                        let graphql_request: async_graphql::Request =
                            match serde_json::from_value(payload) {
                                Ok(req) => req,
                                Err(e) => {
                                    error!("Failed to parse GraphQL request: {}", e);
                                    if tx
                                        .send(ProtocolMessage::error(
                                            subscription_id,
                                            vec![format!("Invalid GraphQL request: {}", e)],
                                        ))
                                        .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                }
                            };

                        // Get schema
                        let schema = match crate::graphql::get_schema() {
                            Ok(s) => s,
                            Err(e) => {
                                error!("Failed to get GraphQL schema: {:?}", e);
                                if tx
                                    .send(ProtocolMessage::error(
                                        subscription_id,
                                        vec![format!("Schema error: {:?}", e)],
                                    ))
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        };

                        // Create auth context
                        let js_auth_context = if let Some(ref user) = auth_user {
                            crate::auth::JsAuthContext::authenticated(
                                user.user_id.clone(),
                                user.email.clone(),
                                user.name.clone(),
                                user.provider.clone(),
                                user.is_admin,
                                user.is_editor,
                            )
                        } else {
                            crate::auth::JsAuthContext::anonymous()
                        };

                        // Check subscription limit
                        let mut state = subscription_state.lock().await;
                        if state.subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_CONNECTION {
                            warn!(
                                "Maximum subscriptions ({}) reached",
                                MAX_SUBSCRIPTIONS_PER_CONNECTION
                            );
                            if tx
                                .send(ProtocolMessage::error(
                                    subscription_id,
                                    vec!["Maximum subscriptions reached".to_string()],
                                ))
                                .is_err()
                            {
                                break;
                            }
                            continue;
                        }

                        // Spawn subscription task
                        let sub_id = subscription_id.clone();
                        let tx_clone = tx.clone();
                        let handle = tokio::spawn(async move {
                            debug!("Starting subscription: {}", sub_id);
                            let mut stream = Box::pin(
                                schema.execute_stream(graphql_request.data(js_auth_context)),
                            );

                            while let Some(response) = stream.next().await {
                                let payload = match serde_json::to_value(&response) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        error!("Failed to serialize response: {}", e);
                                        continue;
                                    }
                                };

                                if tx_clone
                                    .send(ProtocolMessage::next(sub_id.clone(), payload))
                                    .is_err()
                                {
                                    debug!("Subscription {} - client disconnected", sub_id);
                                    break;
                                }
                            }

                            debug!("Subscription {} completed", sub_id);
                            let _ = tx_clone.send(ProtocolMessage::complete(sub_id.clone()));
                        });

                        if !state.add(subscription_id.clone(), handle) {
                            error!("Failed to add subscription");
                        } else {
                            info!("Started subscription: {}", subscription_id);
                        }
                    }

                    MessageType::Complete => {
                        if let Some(id) = msg.id {
                            debug!("Completing subscription: {}", id);
                            let mut state = subscription_state.lock().await;
                            if let Some(handle) = state.remove(&id) {
                                handle.abort();
                                info!("Aborted subscription: {}", id);
                            }
                        }
                    }

                    _ => {
                        warn!("Unexpected message type: {:?}", msg_type);
                    }
                }
            }

            Ok(Message::Close(_)) => {
                info!("WebSocket close received");
                break;
            }

            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                // Handled automatically by axum
            }

            Ok(Message::Binary(_)) => {
                warn!("Received binary message, ignoring");
            }

            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
        }
    }

    // Cleanup
    info!("Cleaning up WebSocket connection");
    subscription_state.lock().await.abort_all().await;
    ping_task.abort();
    send_task.abort();
}
