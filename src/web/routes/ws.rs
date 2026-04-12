use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Extension;
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::agent::events::AgentEvent;
use crate::agent::runtime::AgentRuntime;

#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
pub enum ClientMessage {
    #[serde(rename = "session.send")]
    SessionSend {
        request_id: String,
        session_id: String,
        content: String,
        client_message_id: Option<String>,
    },
    #[serde(rename = "session.cancel")]
    SessionCancel {
        request_id: String,
        session_id: String,
    },
    #[serde(rename = "session.subscribe")]
    SessionSubscribe {
        request_id: String,
        session_id: String,
        #[serde(default)]
        last_seq: Option<u64>,
    },
    #[serde(rename = "session.unsubscribe")]
    SessionUnsubscribe {
        request_id: String,
        session_id: String,
    },
    #[serde(rename = "checkpoint.list")]
    CheckpointList {
        request_id: String,
        session_id: String,
    },
    #[serde(rename = "checkpoint.create")]
    CheckpointCreate {
        request_id: String,
        session_id: String,
    },
    #[serde(rename = "terminal.write")]
    TerminalWrite {
        request_id: String,
        session_id: String,
        data: String,
    },
    #[serde(rename = "terminal.resize")]
    TerminalResize {
        request_id: String,
        session_id: String,
        cols: u16,
        rows: u16,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "method")]
pub enum ServerMessage {
    #[serde(rename = "rpc.result")]
    RpcResult {
        request_id: String,
        result: serde_json::Value,
    },
    #[serde(rename = "rpc.error")]
    RpcError {
        request_id: String,
        code: i32,
        error: String,
    },
    #[serde(rename = "session.event")]
    SessionEvent {
        session_id: String,
        sequence: u64,
        #[serde(skip)]
        event: Box<AgentEvent>,
    },
}

pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, agent))
}

async fn handle_socket(socket: WebSocket, agent: Arc<AgentRuntime>) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMessage>();

    let writer = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                maybe_message = out_rx.recv() => {
                    let Some(message) = maybe_message else { break; };
                    let payload = match serde_json::to_string(&message) {
                        Ok(text) => text,
                        Err(err) => {
                            warn!("failed to serialize websocket response: {}", err);
                            continue;
                        }
                    };
                    if ws_tx.send(Message::Text(payload)).await.is_err() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    if ws_tx.send(Message::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut subscription_tasks: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();

    while let Some(message) = ws_rx.next().await {
        let Ok(message) = message else {
            break;
        };

        match message {
            Message::Text(text) => {
                let parsed = serde_json::from_str::<ClientMessage>(&text);
                let Ok(request) = parsed else {
                    let _ = out_tx.send(ServerMessage::RpcError {
                        request_id: "unknown".to_string(),
                        code: -32700,
                        error: "Invalid JSON-RPC payload".to_string(),
                    });
                    continue;
                };

                match request {
                    ClientMessage::SessionSend {
                        request_id,
                        session_id,
                        content,
                        client_message_id,
                    } => {
                        let message_id = client_message_id.unwrap_or_else(|| Uuid::new_v4().to_string());
                        let result = agent
                            .enqueue_message(&session_id, &message_id, &content, &[], None, &[])
                            .await
                            .and_then(|(queue_item, created)| {
                                let started = agent
                                    .clone()
                                    .spawn_next_queued_message_run(session_id.clone(), "websocket".to_string())?;
                                Ok((queue_item, created, started))
                            });

                        match result {
                            Ok((queue_item, created, started_queue_id)) => {
                                let started_this_message =
                                    started_queue_id.as_deref() == Some(queue_item.id.as_str());
                                let _ = out_tx.send(ServerMessage::RpcResult {
                                    request_id,
                                    result: serde_json::json!({
                                        "accepted": true,
                                        "session_id": session_id,
                                        "status": if started_this_message { "started" } else if created { "queued" } else { "duplicate" },
                                        "queued": !started_this_message,
                                        "queue_position": if started_this_message { serde_json::Value::Null } else { serde_json::json!(queue_item.position) }
                                    }),
                                });
                            }
                            Err(err) => {
                                let _ = out_tx.send(ServerMessage::RpcError {
                                    request_id,
                                    code: -32000,
                                    error: err.to_string(),
                                });
                            }
                        }
                    }
                    ClientMessage::SessionCancel {
                        request_id,
                        session_id,
                    } => {
                        agent.cancel_session(&session_id);
                        agent.cancel_subagents_for_parent(&session_id).await;
                        let _ = out_tx.send(ServerMessage::RpcResult {
                            request_id,
                            result: serde_json::json!({
                                "success": true,
                                "session_id": session_id
                            }),
                        });
                    }
                    ClientMessage::SessionSubscribe {
                        request_id,
                        session_id,
                        last_seq,
                    } => {
                        if let Some(task) = subscription_tasks.remove(&session_id) {
                            task.abort();
                        }
                        let since = last_seq.unwrap_or(0);
                        let out_tx_clone = out_tx.clone();
                        let agent_clone = agent.clone();
                        let stream_session_id = session_id.clone();
                        let task = tokio::spawn(async move {
                            stream_session_events(agent_clone, session_id, since, out_tx_clone).await;
                        });
                        subscription_tasks.insert(stream_session_id.clone(), task);
                        let _ = out_tx.send(ServerMessage::RpcResult {
                            request_id,
                            result: serde_json::json!({
                                "subscribed": true,
                                "session_id": stream_session_id,
                                "last_seq": since
                            }),
                        });
                    }
                    ClientMessage::SessionUnsubscribe {
                        request_id,
                        session_id,
                    } => {
                        if let Some(task) = subscription_tasks.remove(&session_id) {
                            task.abort();
                        }
                        let _ = out_tx.send(ServerMessage::RpcResult {
                            request_id,
                            result: serde_json::json!({
                                "unsubscribed": true,
                                "session_id": session_id
                            }),
                        });
                    }
                    ClientMessage::CheckpointList {
                        request_id,
                        session_id,
                    } => match agent.list_checkpoints(&session_id).await {
                        Ok(items) => {
                            let _ = out_tx.send(ServerMessage::RpcResult {
                                request_id,
                                result: serde_json::json!({ "checkpoints": items }),
                            });
                        }
                        Err(err) => {
                            let _ = out_tx.send(ServerMessage::RpcError {
                                request_id,
                                code: -32000,
                                error: err.to_string(),
                            });
                        }
                    },
                    ClientMessage::CheckpointCreate {
                        request_id,
                        session_id,
                    } => {
                        let _ = out_tx.send(ServerMessage::RpcResult {
                            request_id,
                            result: serde_json::json!({
                                "created": false,
                                "session_id": session_id,
                                "message": "Manual checkpoint creation is not implemented yet"
                            }),
                        });
                    }
                    ClientMessage::TerminalWrite {
                        request_id,
                        session_id,
                        data,
                    } => {
                        debug!(
                            "terminal.write ignored for session {} ({} bytes)",
                            session_id,
                            data.len()
                        );
                        let _ = out_tx.send(ServerMessage::RpcError {
                            request_id,
                            code: -32601,
                            error: "terminal.write is not implemented".to_string(),
                        });
                    }
                    ClientMessage::TerminalResize {
                        request_id,
                        session_id,
                        cols,
                        rows,
                    } => {
                        debug!(
                            "terminal.resize ignored for session {} ({}x{})",
                            session_id,
                            cols,
                            rows
                        );
                        let _ = out_tx.send(ServerMessage::RpcError {
                            request_id,
                            code: -32601,
                            error: "terminal.resize is not implemented".to_string(),
                        });
                    }
                }
            }
            Message::Close(_) => break,
            Message::Ping(payload) => {
                let _ = out_tx.send(ServerMessage::RpcResult {
                    request_id: "ping".to_string(),
                    result: serde_json::json!({ "pong": payload.len() }),
                });
            }
            Message::Pong(_) | Message::Binary(_) => {}
        }
    }

    for (_, task) in subscription_tasks.drain() {
        task.abort();
    }
    drop(out_tx);
    let _ = writer.await;
}

async fn stream_session_events(
    agent: Arc<AgentRuntime>,
    session_id: String,
    from_sequence: u64,
    out_tx: mpsc::UnboundedSender<ServerMessage>,
) {
    let (replay, mut rx) = match agent.subscribe_to_events_from(&session_id, from_sequence) {
        Ok(v) => v,
        Err(err) => {
            let _ = out_tx.send(ServerMessage::RpcError {
                request_id: "session.subscribe".to_string(),
                code: -32000,
                error: err.to_string(),
            });
            return;
        }
    };

    let mut cursor = from_sequence;

    for event in replay {
        if event.session_id() != session_id || event.sequence() <= cursor {
            continue;
        }
        cursor = event.sequence();
        if out_tx
            .send(ServerMessage::SessionEvent {
                session_id: session_id.clone(),
                sequence: event.sequence(),
                event: Box::new(event),
            })
            .is_err()
        {
            return;
        }
    }

    loop {
        match rx.recv().await {
            Ok(event) => {
                if event.session_id() != session_id || event.sequence() <= cursor {
                    continue;
                }
                cursor = event.sequence();
                if out_tx
                    .send(ServerMessage::SessionEvent {
                        session_id: session_id.clone(),
                        sequence: event.sequence(),
                        event: Box::new(event),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}
