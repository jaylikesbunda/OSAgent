//! Minimal Chrome DevTools Protocol client.
//!
//! One WebSocket to the browser endpoint, flattened target sessions (every
//! command may carry a `sessionId`), request/response correlation by id and
//! a broadcast channel for events. The browser tool only needs a few dozen
//! commands, so this is hand-rolled rather than pulling in a generated
//! protocol crate.

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    /// Flattened target session the event came from, if any.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CdpError {
    /// The browser answered with a protocol error.
    Protocol { code: i64, message: String },
    Timeout(String),
    Closed,
    Transport(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CdpError::Protocol { code, message } => write!(f, "{message} (cdp {code})"),
            CdpError::Timeout(method) => write!(f, "timed out waiting for {method}"),
            CdpError::Closed => write!(f, "browser connection closed"),
            CdpError::Transport(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for CdpError {}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, CdpError>>>>>;

pub struct CdpClient {
    next_id: AtomicU64,
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    events: broadcast::Sender<CdpEvent>,
    closed: Arc<AtomicBool>,
}

impl CdpClient {
    pub async fn connect(ws_url: &str) -> Result<Arc<Self>, CdpError> {
        let (stream, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(ws_url),
        )
        .await
        .map_err(|_| CdpError::Timeout("devtools websocket".to_string()))?
        .map_err(|error| CdpError::Transport(format!("devtools connect failed: {error}")))?;

        let (mut sink, mut source) = stream.split();
        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<String>();
        let (events, _) = broadcast::channel(2048);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));

        tokio::spawn(async move {
            while let Some(text) = outgoing_rx.recv().await {
                if sink.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        let reader_pending = pending.clone();
        let reader_events = events.clone();
        let reader_closed = closed.clone();
        tokio::spawn(async move {
            while let Some(frame) = source.next().await {
                let text = match frame {
                    Ok(Message::Text(text)) => text,
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                Self::dispatch(&reader_pending, &reader_events, value);
            }
            reader_closed.store(true, Ordering::SeqCst);
            let waiting: Vec<_> = reader_pending
                .lock()
                .expect("cdp pending lock")
                .drain()
                .collect();
            for (_, sender) in waiting {
                let _ = sender.send(Err(CdpError::Closed));
            }
        });

        Ok(Arc::new(Self {
            next_id: AtomicU64::new(1),
            outgoing,
            pending,
            events,
            closed,
        }))
    }

    fn dispatch(pending: &Pending, events: &broadcast::Sender<CdpEvent>, value: Value) {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            let sender = pending.lock().expect("cdp pending lock").remove(&id);
            if let Some(sender) = sender {
                let result = match value.get("error") {
                    Some(error) => Err(CdpError::Protocol {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown protocol error")
                            .to_string(),
                    }),
                    None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = sender.send(result);
            }
            return;
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            let _ = events.send(CdpEvent {
                method: method.to_string(),
                params: value.get("params").cloned().unwrap_or(Value::Null),
                session_id: value
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// Send a command and wait for its response.
    pub async fn send(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session_id {
            message["sessionId"] = json!(session_id);
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("cdp pending lock").insert(id, tx);
        if self.outgoing.send(message.to_string()).is_err() {
            self.pending.lock().expect("cdp pending lock").remove(&id);
            return Err(CdpError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CdpError::Closed),
            Err(_) => {
                self.pending.lock().expect("cdp pending lock").remove(&id);
                Err(CdpError::Timeout(method.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_resolves_pending_response() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _rx) = broadcast::channel(8);
        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert(7, tx);

        CdpClient::dispatch(&pending, &events, json!({"id": 7, "result": {"ok": true}}));

        let result = rx.try_recv().expect("response delivered").expect("ok");
        assert_eq!(result["ok"], true);
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn dispatch_maps_protocol_errors() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _rx) = broadcast::channel(8);
        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert(1, tx);

        CdpClient::dispatch(
            &pending,
            &events,
            json!({"id": 1, "error": {"code": -32000, "message": "No node with given id"}}),
        );

        match rx.try_recv().expect("response delivered") {
            Err(CdpError::Protocol { code, message }) => {
                assert_eq!(code, -32000);
                assert!(message.contains("No node"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn dispatch_broadcasts_events_with_session() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, mut rx) = broadcast::channel(8);

        CdpClient::dispatch(
            &pending,
            &events,
            json!({"method": "Page.loadEventFired", "params": {"timestamp": 1.0}, "sessionId": "S1"}),
        );

        let event = rx.try_recv().expect("event broadcast");
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session_id.as_deref(), Some("S1"));
    }
}
