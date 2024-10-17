use bincode::Result;
use serde::de::Error as SerdeError;
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub enum ClientMessage {
    Join { username: String },
    Send { message: String },
    Leave,
    Heartbeat,
}

impl ClientMessage {
    pub fn from_json(json: &[u8]) -> Result<Self> {
        bincode::deserialize(json)
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
    }

    pub fn parse(json: &str) -> Result<Self> {
        let parts: Vec<&str> = json.split_whitespace().collect();
        match parts[0].to_lowercase().as_str() {
            "join" => {
                if parts.len() == 2 {
                    Ok(ClientMessage::Join {
                        username: parts[1].to_string(),
                    })
                } else {
                    Err(bincode::Error::custom("Invalid join message format"))
                }
            }
            "send" => {
                if parts.len() > 1 {
                    Ok(ClientMessage::Send {
                        message: parts[1..].join(" "),
                    })
                } else {
                    Err(bincode::Error::custom("Invalid send message format"))
                }
            }
            "leave" => {
                if parts.len() == 1 {
                    Ok(ClientMessage::Leave)
                } else {
                    Err(bincode::Error::custom("Invalid leave message format"))
                }
            }
            _ => {
                if parts[0] == "send" {
                    Ok(ClientMessage::Send {
                        message: parts[1..].join(" "),
                    })
                } else {
                    Ok(ClientMessage::Send {
                        message: parts[0..].join(" "),
                    })
                }
            }
        }
    }

    pub fn parse_to_server_message(&self, user: &String) -> ServerMessage {
        match self {
            ClientMessage::Join { username } => ServerMessage {
                from: username.clone(),
                message: "joined the chat".to_string(),
            },
            ClientMessage::Send { message } => ServerMessage {
                from: user.to_string(),
                message: message.clone(),
            },
            ClientMessage::Leave => ServerMessage {
                from: user.to_string(),
                message: "left the chat".to_string(),
            },
            ClientMessage::Heartbeat => ServerMessage {
                from: "server".to_string(),
                message: "heartbeat".to_string(),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ServerMessage {
    pub from: String,
    pub message: String,
}

impl ServerMessage {
    pub fn to_json(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
    }

    pub fn from_json(json: &[u8]) -> Result<Self> {
        bincode::deserialize(json)
    }
}
