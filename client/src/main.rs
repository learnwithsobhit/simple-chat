//! This module implements a WebSocket-based chat client.
//!
//! The client connects to a chat server, allows users to join with a username,
//! send messages, and leave the chat. It handles incoming and outgoing messages,
//! parses user input, and manages the WebSocket connection.
//!
//! Key features:
//! - Asynchronous I/O using tokio
//! - WebSocket communication with tokio-tungstenite
//! - Message serialization and deserialization with bincode
//! - Handling of stdin for user input and stdout for displaying messages
//! - Graceful shutdown on Ctrl+C

use futures_util::{SinkExt, StreamExt};
use log::{error, info};
use std::env;
use std::io::{self, Write};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::signal;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use common::utils::{ClientMessage, ServerMessage};

#[tokio::main]
async fn main() {
    // Initialize the logger (you'll need to add this)
    env_logger::init();

    let url = env::args().nth(1).unwrap_or_else(|| {
        print!("Please enter the client URL (e.g., 127.0.0.1:12345): ");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");
        input.trim().to_string()
    });

    let username = env::args().nth(2).unwrap_or_else(|| {
        print!("Please enter your username (e.g., John): ");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");
        input.trim().to_string()
    });
    let url = format!("ws://{}", url);
    let (ws_stream, _) = connect_async(&url).await.expect("Failed to connect");
    info!("WebSocket handshake has been successfully completed");
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    tokio::spawn(read_stdin(stdin_tx.clone()));
    let (stdout_tx, mut stdout_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(message) = stdout_rx.recv().await {
            let mut stdout = tokio::io::stdout();
            stdout.write_all(message.as_bytes()).await.unwrap();
            stdout.write_all(b"\n").await.unwrap();
        }
    });

    let (mut write, read) = ws_stream.split();

    // Send Join message
    let join_msg = ClientMessage::Join {
        username: username.to_string(),
    };
    let join_msg = bincode::serialize(&join_msg).unwrap();
    {
        if let Err(e) = write
            .send(tokio_tungstenite::tungstenite::Message::Binary(join_msg))
            .await
        {
            println!("Failed to send join message: {}", e);
            return;
        }
    }
    let stdin_to_ws = async {
        while let Some(message) = stdin_rx.recv().await {
            // println!("Sending message to server: {}", message.to_string());
            write.send(message.clone()).await.unwrap();
            if let Ok(msg) = ClientMessage::from_json(&message.into_data()) {
                if msg == ClientMessage::Leave {
                    write.close().await.unwrap();
                    return;
                }
            }
        }
    };

    let ws_to_stdout = {
        read.for_each(|message| async {
            if let Ok(message) = message {
                if message.is_close() {
                    info!("Server closed the connection");
                    std::process::exit(0);
                }
                let server_message = String::from_utf8(message.clone().into_data());
                if let Ok(msg) = ServerMessage::from_json(&message.into_data()) {
                    stdout_tx
                        .send(format!("{}: {}", msg.from, msg.message))
                        .unwrap();
                } else {
                    stdout_tx.send(server_message.unwrap()).unwrap();
                }
            } else {
                error!(
                    "Error receiving message from server: {}",
                    message.err().unwrap()
                );
            }
        })
    };

    tokio::spawn(handle_ctrl_c(stdin_tx.clone()));
    tokio::select! {
        _ = stdin_to_ws => (),
        _ = ws_to_stdout => (),
    }
    info!("Exiting");
    std::process::exit(0);
}

async fn read_stdin(tx: tokio::sync::mpsc::UnboundedSender<Message>) {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut buffer = String::new();
    loop {
        buffer.clear();
        match reader.read_line(&mut buffer).await {
            Ok(0) => continue, // EOF
            Ok(_) => {
                if buffer.trim().is_empty() {
                    continue;
                }
                let input = buffer.trim();
                ClientMessage::parse(input).map_or_else(
                    |error| error!("Failed to parse message: {}", error.to_string()),
                    |message| {
                        message.to_json().map_or_else(
                            |error| error!("Failed to serialize message: {}", error.to_string()),
                            |serialized| {
                                tx.send(Message::binary(serialized)).unwrap();
                            },
                        );
                    },
                );
            }
            Err(e) => {
                error!("Failed to read from stdin: {}", e);
                continue;
            }
        }
    }
}

async fn handle_ctrl_c(tx: tokio::sync::mpsc::UnboundedSender<Message>) {
    signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
    info!("Received Ctrl+C, sending leave message.");
    let leave_msg = ClientMessage::Leave;
    tx.send(Message::binary(leave_msg.to_json().unwrap()))
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;
    use futures_util::SinkExt;

    #[test]
    fn test_client_message_parse_join() {
        let input = "join testuser";
        let expected = ClientMessage::Join {
            username: "testuser".to_string(),
        };
        assert_eq!(ClientMessage::parse(input).unwrap(), expected);
    }

    #[test]
    fn test_client_message_parse_send() {
        let input = "send Hello, world!";
        let expected = ClientMessage::Send {
            message: "Hello, world!".to_string(),
        };
        assert_eq!(ClientMessage::parse(input).unwrap(), expected);
    }

    #[test]
    fn test_client_message_parse_leave() {
        let input = "leave";
        let expected = ClientMessage::Leave;
        assert_eq!(ClientMessage::parse(input).unwrap(), expected);
    }

    #[test]
    fn test_client_message_parse_implicit_send() {
        let input = "Hello, everyone!";
        let expected = ClientMessage::Send {
            message: "Hello, everyone!".to_string(),
        };
        assert_eq!(ClientMessage::parse(input).unwrap(), expected);
    }

    #[test]
    fn test_client_message_serialization() {
        let message = ClientMessage::Join {
            username: "testuser".to_string(),
        };
        let serialized = message.to_json().unwrap();
        let deserialized = ClientMessage::from_json(&serialized).unwrap();
        assert_eq!(message, deserialized);
    }

    #[test]
    #[should_panic(expected = "Invalid join message format")]
    fn test_client_message_parse_invalid_join() {
        ClientMessage::parse("join").unwrap();
    }

    #[tokio::test]
    async fn test_client_message_handling() {
        let (mut tx, mut rx) = mpsc::unbounded();

        // Test Join message
        let join_msg = "join testuser";
        read_and_send_message(join_msg, &mut tx).await;

        if let Some(message) = rx.next().await {
            assert_eq!(
                message,
                Message::Binary(
                    ClientMessage::Join {
                        username: "testuser".to_string()
                    }
                    .to_json()
                    .unwrap()
                )
            );
        } else {
            panic!("No message received for join");
        }

        // Test Send message
        let send_msg = "send Hello, world!";
        read_and_send_message(send_msg, &mut tx).await;

        if let Some(message) = rx.next().await {
            assert_eq!(
                message,
                Message::Binary(
                    ClientMessage::Send {
                        message: "Hello, world!".to_string()
                    }
                    .to_json()
                    .unwrap()
                )
            );
        } else {
            panic!("No message received for send");
        }

        // Test Leave message
        let leave_msg = "leave";
        read_and_send_message(leave_msg, &mut tx).await;

        if let Some(message) = rx.next().await {
            assert_eq!(
                message,
                Message::Binary(ClientMessage::Leave.to_json().unwrap())
            );
        } else {
            panic!("No message received for leave");
        }
    }

    async fn read_and_send_message(input: &str, tx: &mut mpsc::UnboundedSender<Message>) {
        let message = ClientMessage::parse(input).unwrap();
        tx.send(Message::Binary(message.to_json().unwrap()))
            .await
            .unwrap();
    }

    #[test]
    fn test_server_message_deserialization() {
        let server_message = ServerMessage {
            from: "Server".to_string(),
            message: "Welcome to the chat!".to_string(),
        };
        let serialized = bincode::serialize(&server_message).unwrap();
        let deserialized = ServerMessage::from_json(&serialized).unwrap();
        assert_eq!(server_message.from, deserialized.from);
        assert_eq!(server_message.message, deserialized.message);
    }
}
