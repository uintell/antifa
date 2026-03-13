use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Peer {
    Local,
    Remote(String),
    System,
}

impl Peer {
    #[allow(dead_code)]
    pub fn label(&self) -> &str {
        match self {
            Peer::Local => "you",
            Peer::Remote(_) => "peer",
            Peer::System => "system",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub from: Peer,
    pub body: String,
    pub timestamp: u64,
}

impl ChatMessage {
    pub fn system(body: impl Into<String>) -> Self {
        Self {
            from: Peer::System,
            body: body.into(),
            timestamp: now(),
        }
    }

    pub fn local(body: impl Into<String>) -> Self {
        Self {
            from: Peer::Local,
            body: body.into(),
            timestamp: now(),
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
