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
use mongodb::bson::doc;
use mongodb::IndexModel;
use std::io::Write;
use std::time::{Duration, Instant};
use std::{env, net::SocketAddr, sync::Arc};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::sync::broadcast;
use tokio::time::interval;
use tokio_tungstenite::tungstenite::protocol::Message;
use mongodb::{Client, options::ClientOptions};
use serde::{Serialize, Deserialize};
use dotenv::dotenv;

type PeerMap = Arc<DashMap<SocketAddr, String>>;

#[derive(Debug, Serialize, Deserialize)]
struct UserData {
    username: String,
    address: String,
    last_seen: i64,
}

// Add this new struct for message storage
#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    from: String,
    content: String,
    timestamp: i64,
}

// Change the trait to use an associated type
trait DatabaseClient: Send + Sync {
    type DatabaseType;
    fn database(&self, name: &str) -> Self::DatabaseType;
}

// Implement for real MongoDB client
impl DatabaseClient for Client {
    type DatabaseType = mongodb::Database;
    fn database(&self, name: &str) -> Self::DatabaseType {
        self.database(name)
    }
}

async fn handle_connection(
    peer_map: PeerMap,
    raw_stream: TcpStream,
    addr: SocketAddr,
    tx: broadcast::Sender<ServerMessage>,
    tx_close: broadcast::Sender<Message>,
    db_client: Arc<dyn DatabaseClient<DatabaseType = mongodb::Database>>,
) {
    info!("Creating index for username");
     // Get MongoDB collection
     let collection = db_client
     .database("chat_db")
     .collection::<UserData>("users");
     collection
        .create_index(
            IndexModel::builder().keys(doc! { "username": 1 }).build(),
        )
    .await
    .expect("Failed to create index");

    let collection_messages = db_client
        .database("chat_db")
        .collection::<ChatMessage>("messages");

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
                        let filter = doc! { "username": username.value() };
                        let update = doc! { "$set": { "last_seen": chrono::Utc::now().timestamp() } };
                        if let Err(e) = collection.update_one(filter, update).await {
                            log::error!("Failed to update last_seen: {}", e);
                        }
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
                            let filter = doc! { "username": username.value() };
                        let update = doc! { "$set": { "last_seen": chrono::Utc::now().timestamp() } };
                        if let Err(e) = collection.update_one(filter, update).await {
                                log::error!("Failed to update last_seen: {}", e);
                            }
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

                                // Store user data in MongoDB
                                let user_data = UserData {
                                    username: username.clone(),
                                    address: addr.to_string(),
                                    last_seen: chrono::Utc::now().timestamp(),
                                };
                                
                                if let Err(e) = collection.insert_one(user_data).await {
                                    log::error!("Failed to store user data: {}", e);
                                }

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
                             // Add this block to store messages
                        if let ClientMessage::Send { message } = &msg {
                            if let Some(username) = peer_map.get(&addr) {
                                let chat_message = ChatMessage {
                                    from: username.clone(),
                                    content: message.clone(),
                                    timestamp: chrono::Utc::now().timestamp(),
                                };
                                
                                    if let Err(e) = collection_messages.insert_one(chat_message).await {
                                    log::error!("Failed to store message: {}", e);
                                }
                                }
                            }
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
    dotenv().ok(); // Load .env file
    // Setup MongoDB connection
    let mongo_uri = env::var("MONGODB_URI").unwrap_or_else(|_| "mongodb://localhost:27017".to_string());
    info!("MongoDB URI: {}", mongo_uri);
    let client_options = ClientOptions::parse(&mongo_uri)
        .await
        .expect("Failed to parse MongoDB options");
    let db_client = Arc::new(Client::with_options(client_options)
        .expect("Failed to connect to MongoDB"));

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
                let db_client = db_client.clone();
                tokio::spawn(handle_connection(
                    state.clone(),
                    stream,
                    addr,
                    tx.clone(),
                    tx_close.clone(),
                    db_client.clone(),
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
