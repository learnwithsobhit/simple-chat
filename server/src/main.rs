//! A simple WebSocket chat server implementation.
//!
//! This module provides a WebSocket-based chat server that allows multiple clients to connect,
//! join the chat with a username, send messages, and leave the chat.
//!
//! # Features
//!
//! - WebSocket-based communication
//! - Username registration
//! - Broadcasting messages to all connected clients
//! - Graceful handling of client disconnections
//!
//! # Examples
//!
//! To run the server:
//!
//! ```bash
//! cargo run --example server 127.0.0.1:12345
//! ```
//!
//! To run a client (in a separate terminal):
//!
//! ```bash
//! cargo run --example client ws://127.0.0.1:12345/
//! ```
//!

use log::info;
use std::{env, net::SocketAddr, sync::Arc};

use bincode::Result;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::protocol::Message;
type PeerMap = Arc<DashMap<SocketAddr, String>>;
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
enum ClientMessage {
    Join { username: String },
    Send { message: String },
    Leave,
}

impl ClientMessage {
    fn from_json(json: &[u8]) -> Result<Self> {
        bincode::deserialize(json)
    }

    fn parse(&self, user: &String) -> ServerMessage {
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
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct ServerMessage {
    from: String,
    message: String,
}

impl ServerMessage {
    fn to_json(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
    }
}

async fn handle_connection(
    peer_map: PeerMap,
    raw_stream: TcpStream,
    addr: SocketAddr,
    tx: broadcast::Sender<ServerMessage>,
) {
    info!("Incoming TCP connection from: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(raw_stream)
        .await
        .expect("Error during the websocket handshake occurred");
    info!("WebSocket connection established: {}", addr);

    let mut rx = tx.subscribe(); // Each client gets a subscription
    let (mut outgoing, mut incoming) = ws_stream.split();
    loop {
        tokio::select! {
            Some(Ok(msg)) = incoming.next() => {
                if msg.is_close() {
                    info!("{} disconnected", addr);
                    if let Some(username) = peer_map.get(&addr) {
                        let _ = tx.send(ClientMessage::Leave.parse(&username));
                    }
                    peer_map.remove(&addr);
                    let _ = outgoing.close().await;
                    break;
                }

                let msg: ClientMessage = ClientMessage::from_json(&msg.into_data()).unwrap();
                if msg == ClientMessage::Leave {
                    if let Some(username) = peer_map.get(&addr) {
                        let _ = tx.send(msg.parse(&username));
                    }
                    peer_map.remove(&addr);
                }
                if let ClientMessage::Join { username } = msg.clone() {
                    if peer_map.iter().any(|entry| entry.value() == &username) {
                        let _ = outgoing
                            .send(Message::text(format!("Sorry, username {} already taken.", username)))
                            .await;
                    } else {

                        peer_map.insert(addr, username.clone());

                        // Notify current client of successful join
                        // Notify current client of successful join with additional instructions
                        let welcome_message = format!(
                            "Welcome to the chat, {}!\n\nYou can interact with Server as follows:\n1. leave - to leave from room.\n2. join <username> - to join to room.\n3. send <MSG> or <MSG> - to send message in the room",
                            username
                        );
                        let _ = outgoing
                            .send(Message::text(welcome_message))
                            .await;
                        if let Some(username) = peer_map.get(&addr) {
                            let _ = tx.send(msg.parse(&username));
                        }
                    }
                } else if let Some(username) = peer_map.get(&addr) {
                        let _ = tx.send(msg.parse(&username));
                }
            },

            Ok(message) = rx.recv() => {

                if let Some(info) = peer_map.get(&addr) {

                    if info.clone() != message.from {
                        let _ = outgoing
                            .send(Message::binary(message.to_json().unwrap()))
                            .await;
                    }
                }
            }

            else => {
                if let Some(username) = peer_map.get(&addr) {
                    let _ = tx.send(ClientMessage::Leave.parse(&username));
                    info!("{:?} disconnected", username);
                }
                peer_map.remove(&addr);
                break;
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());

    let state = Arc::new(DashMap::new());
    // Create the event loop and TCP listener we'll accept connections on.

    // Setup TCP listener for websocket connections

    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    let (tx, _) = broadcast::channel::<ServerMessage>(100); // Broadcast channel for messages

    println!("Listening on: {}", addr);
    tokio::spawn(handle_ctrl_c(tx.clone()));
    // Let's spawn the handling of each connection in a separate task.
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(state.clone(), stream, addr, tx.clone()));
    }
    info!("Server shutdown");
    Ok(())
}

async fn handle_ctrl_c(_tx: broadcast::Sender<ServerMessage>) {
    signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
    info!("Received Ctrl+C, sending leave message.");
    std::process::exit(0);
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_client_message_serialization() {
        let join_msg = ClientMessage::Join {
            username: "Alice".to_string(),
        };
        let serialized = bincode::serialize(&join_msg).unwrap();
        let deserialized: ClientMessage = ClientMessage::from_json(&serialized).unwrap();
        assert_eq!(join_msg, deserialized);

        let send_msg = ClientMessage::Send {
            message: "Hello, world!".to_string(),
        };
        let serialized = bincode::serialize(&send_msg).unwrap();
        let deserialized: ClientMessage = ClientMessage::from_json(&serialized).unwrap();
        assert_eq!(send_msg, deserialized);

        let leave_msg = ClientMessage::Leave;
        let serialized = bincode::serialize(&leave_msg).unwrap();
        let deserialized: ClientMessage = ClientMessage::from_json(&serialized).unwrap();
        assert_eq!(leave_msg, deserialized);
    }

    #[test]
    fn test_client_message_parsing() {
        let join_msg = ClientMessage::Join {
            username: "Alice".to_string(),
        };
        let parsed = join_msg.parse(&"Alice".to_string());
        assert_eq!(
            parsed,
            ServerMessage {
                from: "Alice".to_string(),
                message: "joined the chat".to_string(),
            }
        );

        let send_msg = ClientMessage::Send {
            message: "Hello, world!".to_string(),
        };
        let parsed = send_msg.parse(&"Bob".to_string());
        assert_eq!(
            parsed,
            ServerMessage {
                from: "Bob".to_string(),
                message: "Hello, world!".to_string(),
            }
        );

        let leave_msg = ClientMessage::Leave;
        let parsed = leave_msg.parse(&"Charlie".to_string());
        assert_eq!(
            parsed,
            ServerMessage {
                from: "Charlie".to_string(),
                message: "left the chat".to_string(),
            }
        );
    }

    #[test]
    fn test_server_message_serialization() {
        let msg = ServerMessage {
            from: "Alice".to_string(),
            message: "Hello, everyone!".to_string(),
        };
        let serialized = msg.to_json().unwrap();
        let deserialized: ServerMessage = bincode::deserialize(&serialized).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[tokio::test]
    async fn test_handle_connection() {
        use futures_util::StreamExt;
        use tokio::net::TcpListener;
        use tokio_tungstenite::connect_async;

        // Setup a mock server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let peer_map = Arc::new(DashMap::new());
        let (tx, _) = broadcast::channel::<ServerMessage>(100);

        // Spawn the server handler
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection(peer_map, stream, addr, tx).await;
        });

        // Connect a mock client
        let (ws_stream, _) = connect_async(format!("ws://{}", addr)).await.unwrap();

        let (mut write, mut read) = ws_stream.split();
        // Test joining
        let join_msg = ClientMessage::Join {
            username: "TestUser".to_string(),
        };
        let serialized = bincode::serialize(&join_msg).unwrap();
        write.send(Message::Binary(serialized)).await.unwrap();
        let welcome_message = "Welcome to the chat, TestUser!\n\nYou can interact with Server as follows:\n1. leave - to leave from room.\n2. join <username> - to join to room.\n3. send <MSG> or <MSG> - to send message in the room";
        // Receive the welcome message
        if let Some(Ok(msg)) = read.next().await {
            assert_eq!(msg, Message::Text(welcome_message.to_string()));
        } else {
            panic!("Did not receive welcome message");
        }

        // Test sending a message
        let send_msg = ClientMessage::Send {
            message: "Hello, chat!".to_string(),
        };
        let serialized = bincode::serialize(&send_msg).unwrap();
        write.send(Message::Binary(serialized)).await.unwrap();
        // Test leaving the chat
        let leave_msg = ClientMessage::Leave;
        let serialized = bincode::serialize(&leave_msg).unwrap();
        write.send(Message::Binary(serialized)).await.unwrap();

        // Close the connection
        write.close().await.unwrap();
        handle.abort();
    }

    #[tokio::test]
    async fn test_multiple_clients() {
        use futures_util::StreamExt;
        use tokio::net::TcpListener;
        use tokio_tungstenite::connect_async;

        // Setup a mock server
        let listener = TcpListener::bind("127.0.0.1:12345").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let peer_map = Arc::new(DashMap::new());
        let (tx, _) = broadcast::channel::<ServerMessage>(100);

        // Spawn the server handler
        let server_handle = tokio::spawn(async move {
            while let Ok((stream, client_addr)) = listener.accept().await {
                let peer_map = peer_map.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_connection(peer_map, stream, client_addr, tx).await;
                });
            }
        });

        // Connect two mock clients
        let (alice_stream, _) = connect_async(format!("ws://{}", addr)).await.unwrap();
        let (bob_stream, _) = connect_async(format!("ws://{}", addr)).await.unwrap();

        let (mut alice_write, mut alice_read) = alice_stream.split();
        let (mut bob_write, mut bob_read) = bob_stream.split();
        // Alice joins
        let join_msg = ClientMessage::Join {
            username: "Alice".to_string(),
        };
        let serialized = bincode::serialize(&join_msg).unwrap();
        alice_write.send(Message::Binary(serialized)).await.unwrap();

        // Bob joins
        let join_msg = ClientMessage::Join {
            username: "Bob".to_string(),
        };
        let serialized = bincode::serialize(&join_msg).unwrap();
        bob_write.send(Message::Binary(serialized)).await.unwrap();

        // Receive welcome messages
        alice_read.next().await;
        bob_read.next().await;

        // Alice should receive Bob's message about joining
        if let Some(Ok(Message::Binary(msg))) = alice_read.next().await {
            let server_msg: ServerMessage = bincode::deserialize(&msg).unwrap();
            assert_eq!(server_msg.from, "Bob");
            assert_eq!(server_msg.message, "joined the chat");
        } else {
            panic!("Alice did not receive Bob's message");
        }

        // Alice sends a message
        let send_msg = ClientMessage::Send {
            message: "Hello, Bob!".to_string(),
        };
        let serialized = bincode::serialize(&send_msg).unwrap();
        alice_write.send(Message::Binary(serialized)).await.unwrap();
        // Bob should receive Alice's message
        if let Some(Ok(Message::Binary(msg))) = bob_read.next().await {
            let server_msg: ServerMessage = bincode::deserialize(&msg).unwrap();
            assert_eq!(server_msg.from, "Alice");
            assert_eq!(server_msg.message, "Hello, Bob!");
        } else {
            panic!("Bob did not receive Alice's message");
        }
        // Close connections
        alice_write.close().await.unwrap();
        bob_write.close().await.unwrap();
        server_handle.abort();
    }
}
