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

use common::utils::{ClientMessage, ServerMessage};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use log::info;
use std::io::Write;
use std::time::{Duration, Instant};
use std::{env, net::SocketAddr, sync::Arc};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::broadcast;
use tokio::time::interval;
use tokio_tungstenite::tungstenite::protocol::Message;

type PeerMap = Arc<DashMap<SocketAddr, String>>;

async fn handle_connection(
    peer_map: PeerMap,
    raw_stream: TcpStream,
    addr: SocketAddr,
    tx: broadcast::Sender<ServerMessage>,
    tx_close: broadcast::Sender<Message>,
) {
    info!("Incoming TCP connection from: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(raw_stream)
        .await
        .expect("Error during the websocket handshake occurred");
    info!("WebSocket connection established: {}", addr);

    let mut rx = tx.subscribe(); // Each client gets a subscription
    let mut rx_close = tx_close.subscribe();
    let (mut outgoing, mut incoming) = ws_stream.split();
    let mut last_heartbeat = Instant::now();
    let heartbeat_interval = Duration::from_secs(3);
    let mut heartbeat_check = interval(Duration::from_secs(1)); // Check every second

    loop {
        tokio::select! {
            Some(Ok(msg)) = incoming.next() => {
                if msg.is_close() {
                    info!("{} disconnected", addr);
                    if let Some(username) = peer_map.get(&addr) {
                        let _ = tx.send(ClientMessage::Leave.parse_to_server_message(&username));
                    }
                    peer_map.remove(&addr);
                    let _ = outgoing.close().await;
                    break;
                }

                if let Ok(msg) = ClientMessage::from_json(&msg.into_data())
                {
                    if msg == ClientMessage::Leave {
                        if let Some(username) = peer_map.get(&addr) {
                            let _ = tx.send(msg.parse_to_server_message(&username));
                        }
                        peer_map.remove(&addr);
                    }
                    if msg == ClientMessage::Heartbeat {
                        last_heartbeat = Instant::now();
                        continue;
                    }
                    if let ClientMessage::Join { username } = msg.clone() {
                        if peer_map.iter().any(|entry| entry.value() == &username) {
                            let _ = outgoing
                                .send(Message::text(format!("Sorry, username {} already taken.", username)))
                                .await;
                        } else if peer_map.contains_key(&addr) {
                                let joined_user = peer_map.get(&addr).unwrap().clone();
                                let _ = outgoing
                                .send(Message::text(format!("{} already joined in chat ,you can leave and join as {}", joined_user,username)))
                                .await;
                            }else{
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
                                    let _ = tx.send(msg.parse_to_server_message(&username));
                                }
                            }
                        } else if let Some(username) = peer_map.get(&addr) {
                            let _ = tx.send(msg.parse_to_server_message(&username));
                    }
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
            Ok(message) = rx_close.recv() => { //this is for ctrl+c
                if peer_map.get(&addr).is_some() {
                    let _ = outgoing
                        .send(message)
                        .await;
                    peer_map.remove(&addr);
                    let _ = outgoing.close().await;
                    break;
                }
            }
            _ = heartbeat_check.tick() => {
                if last_heartbeat.elapsed() > heartbeat_interval {
                    info!("No heartbeat received from {} in 30 seconds, closing connection", addr);
                    if let Some(username) = peer_map.get(&addr) {
                        let _ = tx.send(ClientMessage::Leave.parse_to_server_message(&username));
                    }
                    peer_map.remove(&addr);
                    let _ = outgoing.close().await;
                    break;
                }
            }
            else => {
                if let Some(username) = peer_map.get(&addr) {
                    let _ = tx.send(ClientMessage::Leave.parse_to_server_message(&username));
                    info!("{:?} disconnected", username);
                }
                peer_map.remove(&addr);
                let _ = outgoing.close().await;
                break;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let addr = env::args().nth(1).unwrap_or_else(|| {
        print!("Please enter the server URL (e.g., 0.0.0.0:12345): ");
        std::io::stdout().flush().unwrap();
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");
        input.trim().to_string()
    });

    let state = Arc::new(DashMap::new());
    // Create the event loop and TCP listener we'll accept connections on.

    // Setup TCP listener for websocket connections

    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    let (tx, _) = broadcast::channel::<ServerMessage>(100); // Broadcast channel for messages
    let (tx_close, _) = broadcast::channel::<Message>(100);
    println!("****************************************************************");
    println!(
        "* Listening on: {}                                  *",
        addr
    );
    println!("* To connect to chat use below command in separate terminal:   *");
    println!("* cargo run --bin client <ip>:<port> <username>                *");
    println!("****************************************************************");
    loop {
        tokio::select! {
            Ok((stream, addr)) = listener.accept() => {
                tokio::spawn(handle_connection(
                    state.clone(),
                    stream,
                    addr,
                    tx.clone(),
                    tx_close.clone(),
                ));
            }
            _ = handle_ctrl_c(tx_close.clone()) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                break;
            }
        }
    }
}

async fn handle_ctrl_c(tx_close: broadcast::Sender<Message>) {
    signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
    info!("Received Ctrl+C, sending leave message.");
    let _ = tx_close.send(Message::Close(None));
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_client_message_serialization() {
        let join_msg = ClientMessage::Join {
            username: "Alice".to_string(),
        };
        let serialized = join_msg.to_json().unwrap();
        let deserialized: ClientMessage = ClientMessage::from_json(&serialized).unwrap();
        assert_eq!(join_msg, deserialized);

        let send_msg = ClientMessage::Send {
            message: "Hello, world!".to_string(),
        };
        let serialized = send_msg.to_json().unwrap();
        let deserialized: ClientMessage = ClientMessage::from_json(&serialized).unwrap();
        assert_eq!(send_msg, deserialized);

        let leave_msg = ClientMessage::Leave;
        let serialized = leave_msg.to_json().unwrap();
        let deserialized: ClientMessage = ClientMessage::from_json(&serialized).unwrap();
        assert_eq!(leave_msg, deserialized);
    }

    #[test]
    fn test_client_message_parsing() {
        let join_msg = ClientMessage::Join {
            username: "Alice".to_string(),
        };
        let parsed = join_msg.parse_to_server_message(&"Alice".to_string());
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
        let parsed = send_msg.parse_to_server_message(&"Bob".to_string());
        assert_eq!(
            parsed,
            ServerMessage {
                from: "Bob".to_string(),
                message: "Hello, world!".to_string(),
            }
        );

        let leave_msg = ClientMessage::Leave;
        let parsed = leave_msg.parse_to_server_message(&"Charlie".to_string());
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
        let deserialized: ServerMessage = ServerMessage::from_json(&serialized).unwrap();
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
        let (tx_close, _) = broadcast::channel::<Message>(100);
        // Spawn the server handler
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection(peer_map, stream, addr, tx, tx_close).await;
        });

        // Connect a mock client
        let (ws_stream, _) = connect_async(format!("ws://{}", addr)).await.unwrap();

        let (mut write, mut read) = ws_stream.split();
        // Test joining
        let join_msg = ClientMessage::Join {
            username: "TestUser".to_string(),
        };
        let serialized = join_msg.to_json().unwrap();
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
        let serialized = send_msg.to_json().unwrap();
        write.send(Message::Binary(serialized)).await.unwrap();
        // Test leaving the chat
        let leave_msg = ClientMessage::Leave;
        let serialized = leave_msg.to_json().unwrap();
        write.send(Message::Binary(serialized)).await.unwrap();

        // Close the connection
        write.close().await.unwrap();
        handle.abort();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
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
        let (tx_close, _) = broadcast::channel::<Message>(100);

        // Spawn the server handler
        let server_handle = tokio::spawn(async move {
            while let Ok((stream, client_addr)) = listener.accept().await {
                let peer_map = peer_map.clone();
                let tx = tx.clone();
                let tx_close = tx_close.clone();
                tokio::spawn(async move {
                    handle_connection(peer_map, stream, client_addr, tx, tx_close).await;
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
        let serialized = join_msg.to_json().unwrap();
        alice_write.send(Message::Binary(serialized)).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        // Bob joins
        let join_msg = ClientMessage::Join {
            username: "Bob".to_string(),
        };
        let serialized = join_msg.to_json().unwrap();
        bob_write.send(Message::Binary(serialized)).await.unwrap();

        // Receive welcome messages
        alice_read.next().await;
        bob_read.next().await;

        // Alice should receive Bob's message about joining
        if let Some(Ok(Message::Binary(msg))) = alice_read.next().await {
            let server_msg: ServerMessage = ServerMessage::from_json(&msg).unwrap();
            assert_eq!(server_msg.from, "Bob");
            assert_eq!(server_msg.message, "joined the chat");
        } else {
            panic!("Alice did not receive Bob's message");
        }

        // Alice sends a message
        let send_msg = ClientMessage::Send {
            message: "Hello, Bob!".to_string(),
        };
        let serialized = send_msg.to_json().unwrap();
        alice_write.send(Message::Binary(serialized)).await.unwrap();
        // Bob should receive Alice's message
        if let Some(Ok(Message::Binary(msg))) = bob_read.next().await {
            let server_msg: ServerMessage = ServerMessage::from_json(&msg).unwrap();
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
