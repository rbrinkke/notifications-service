use crate::models::{ClientMessage, ConnectedMessage, PongMessage};
use crate::ws::manager::{ConnectionManager, WsSender};
use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

/// Handle a single WebSocket connection
pub async fn handle_connection(
    socket: WebSocket,
    user_id: Uuid,
    manager: ConnectionManager,
) {
    let connection_start = Instant::now();
    let connection_id = Uuid::new_v4();

    trace!(
        user_id = %user_id,
        connection_id = %connection_id,
        "WebSocket upgrade received, initializing connection..."
    );

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Create channel for sending messages to this connection
    let (tx, mut rx): (WsSender, mpsc::UnboundedReceiver<String>) = mpsc::unbounded_channel();

    // Register connection
    trace!(
        user_id = %user_id,
        connection_id = %connection_id,
        "Registering connection with ConnectionManager..."
    );
    manager.connect(user_id, tx.clone()).await;

    // Send welcome message
    trace!(
        user_id = %user_id,
        connection_id = %connection_id,
        "Sending welcome message..."
    );
    let welcome = serde_json::to_string(&ConnectedMessage::new(user_id)).unwrap();
    trace!("Welcome payload: {}", welcome);

    if let Err(e) = ws_sender.send(Message::Text(welcome.into())).await {
        error!(
            user_id = %user_id,
            connection_id = %connection_id,
            error = %e,
            "Failed to send welcome message, closing connection"
        );
        manager.disconnect(user_id, &tx).await;
        return;
    }

    info!(
        user_id = %user_id,
        connection_id = %connection_id,
        "✓ WebSocket connection established"
    );

    // Spawn task to forward messages from channel to WebSocket
    let user_id_send = user_id;
    let conn_id_send = connection_id;
    let send_task = tokio::spawn(async move {
        let mut msg_count: u64 = 0;
        while let Some(msg) = rx.recv().await {
            msg_count += 1;
            trace!(
                user_id = %user_id_send,
                connection_id = %conn_id_send,
                message_number = msg_count,
                payload_len = msg.len(),
                "Forwarding message to WebSocket"
            );
            if ws_sender.send(Message::Text(msg.into())).await.is_err() {
                debug!(
                    user_id = %user_id_send,
                    connection_id = %conn_id_send,
                    messages_sent = msg_count,
                    "WebSocket send failed, stopping forwarder"
                );
                break;
            }
        }
        debug!(
            user_id = %user_id_send,
            connection_id = %conn_id_send,
            total_messages_sent = msg_count,
            "Message forwarder task ended"
        );
    });

    // Handle incoming messages
    let mut recv_count: u64 = 0;
    while let Some(result) = ws_receiver.next().await {
        recv_count += 1;
        match result {
            Ok(Message::Text(text)) => {
                trace!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    message_number = recv_count,
                    payload_len = text.len(),
                    "Received text message from client"
                );
                handle_client_message(&text, user_id, connection_id, &tx).await;
            }
            Ok(Message::Ping(data)) => {
                trace!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    data_len = data.len(),
                    "Received ping (auto-pong by axum)"
                );
            }
            Ok(Message::Pong(data)) => {
                trace!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    data_len = data.len(),
                    "Received pong from client"
                );
            }
            Ok(Message::Binary(data)) => {
                debug!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    data_len = data.len(),
                    "Received binary message (ignoring)"
                );
            }
            Ok(Message::Close(frame)) => {
                let reason = frame.as_ref().map(|f| f.reason.to_string()).unwrap_or_default();
                let code = frame.as_ref().map(|f| f.code.into()).unwrap_or(0u16);
                info!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    close_code = code,
                    close_reason = %reason,
                    "Client initiated close"
                );
                break;
            }
            Err(e) => {
                warn!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    error = %e,
                    "WebSocket error, closing connection"
                );
                break;
            }
        }
    }

    // Cleanup
    let connection_duration = connection_start.elapsed();
    trace!(
        user_id = %user_id,
        connection_id = %connection_id,
        "Aborting send task..."
    );
    send_task.abort();

    trace!(
        user_id = %user_id,
        connection_id = %connection_id,
        "Disconnecting from ConnectionManager..."
    );
    manager.disconnect(user_id, &tx).await;

    info!(
        user_id = %user_id,
        connection_id = %connection_id,
        duration_secs = connection_duration.as_secs(),
        messages_received = recv_count,
        "WebSocket connection closed"
    );
}

async fn handle_client_message(text: &str, user_id: Uuid, connection_id: Uuid, tx: &WsSender) {
    trace!(
        user_id = %user_id,
        connection_id = %connection_id,
        raw_message = %text,
        "Parsing client message..."
    );

    let msg: Result<ClientMessage, _> = serde_json::from_str(text);

    match msg {
        Ok(ClientMessage::Ping) => {
            debug!(
                user_id = %user_id,
                connection_id = %connection_id,
                "Received application-level ping, sending pong"
            );
            let pong = serde_json::to_string(&PongMessage::default()).unwrap();
            trace!("Pong payload: {}", pong);
            match tx.send(pong) {
                Ok(_) => trace!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    "Pong queued for sending"
                ),
                Err(e) => warn!(
                    user_id = %user_id,
                    connection_id = %connection_id,
                    error = %e,
                    "Failed to queue pong response"
                ),
            }
        }
        Ok(ClientMessage::SyncComplete { notification_ids }) => {
            debug!(
                user_id = %user_id,
                connection_id = %connection_id,
                sync_count = notification_ids.len(),
                "Client confirmed sync complete"
            );
            if !notification_ids.is_empty() {
                trace!(
                    user_id = %user_id,
                    "Synced notification IDs: {:?}",
                    notification_ids
                );
            }
        }
        Err(e) => {
            warn!(
                user_id = %user_id,
                connection_id = %connection_id,
                error = %e,
                raw_text = %text,
                "Failed to parse client message"
            );
        }
    }
}
