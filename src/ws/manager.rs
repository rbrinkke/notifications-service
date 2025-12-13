use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, trace, warn};
use uuid::Uuid;

/// Message to send to a WebSocket connection
pub type WsMessage = String;

/// Sender half for a WebSocket connection
pub type WsSender = mpsc::UnboundedSender<WsMessage>;

/// Manages all active WebSocket connections, keyed by user_id
#[derive(Default, Clone)]
pub struct ConnectionManager {
    /// Map of user_id -> list of WebSocket senders (multi-device support)
    connections: Arc<RwLock<HashMap<Uuid, Vec<WsSender>>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        debug!("Creating new ConnectionManager");
        Self::default()
    }

    /// Register a new WebSocket connection for a user
    pub async fn connect(&self, user_id: Uuid, sender: WsSender) {
        let start = Instant::now();
        trace!(user_id = %user_id, "Acquiring write lock for connect...");

        let mut connections = self.connections.write().await;
        let lock_time = start.elapsed();

        connections.entry(user_id).or_default().push(sender);

        let user_connections = connections.get(&user_id).map(|v| v.len()).unwrap_or(0);
        let total_users = connections.len();
        let total_connections: usize = connections.values().map(|v| v.len()).sum();

        info!(
            user_id = %user_id,
            user_connection_count = user_connections,
            total_users = total_users,
            total_connections = total_connections,
            lock_ms = lock_time.as_millis() as u64,
            "User connected"
        );

        trace!(
            "ConnectionManager state after connect: {} users, {} total connections",
            total_users, total_connections
        );
    }

    /// Remove a WebSocket connection for a user
    pub async fn disconnect(&self, user_id: Uuid, sender: &WsSender) {
        let start = Instant::now();
        trace!(user_id = %user_id, "Acquiring write lock for disconnect...");

        let mut connections = self.connections.write().await;
        let lock_time = start.elapsed();

        if let Some(senders) = connections.get_mut(&user_id) {
            let before_count = senders.len();

            // Remove the specific sender by comparing pointer addresses
            senders.retain(|s| !std::ptr::eq(s, sender));

            let after_count = senders.len();
            let removed = before_count - after_count;

            if senders.is_empty() {
                connections.remove(&user_id);
                info!(
                    user_id = %user_id,
                    connections_removed = removed,
                    lock_ms = lock_time.as_millis() as u64,
                    "User fully disconnected (no more connections)"
                );
            } else {
                debug!(
                    user_id = %user_id,
                    remaining_connections = senders.len(),
                    connections_removed = removed,
                    lock_ms = lock_time.as_millis() as u64,
                    "Connection removed, user still has other connections"
                );
            }
        } else {
            warn!(
                user_id = %user_id,
                "Disconnect called but user not found in connections map"
            );
        }

        let total_users = connections.len();
        let total_connections: usize = connections.values().map(|v| v.len()).sum();
        trace!(
            "ConnectionManager state after disconnect: {} users, {} total connections",
            total_users, total_connections
        );
    }

    /// Check if a user has any active connections
    pub async fn is_connected(&self, user_id: &Uuid) -> bool {
        trace!(user_id = %user_id, "Checking if user is connected...");
        let start = Instant::now();

        let connections = self.connections.read().await;
        let lock_time = start.elapsed();

        let result = connections
            .get(user_id)
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        trace!(
            user_id = %user_id,
            is_connected = result,
            lock_ms = lock_time.as_millis() as u64,
            "Connection check complete"
        );

        result
    }

    /// Get number of connections for a specific user
    pub async fn connection_count(&self, user_id: &Uuid) -> usize {
        let connections = self.connections.read().await;
        connections.get(user_id).map(|v| v.len()).unwrap_or(0)
    }

    /// Send a message to all connections for a user
    /// Returns the number of successful sends
    pub async fn send_to_user(&self, user_id: &Uuid, message: &str) -> usize {
        let start = Instant::now();
        trace!(
            user_id = %user_id,
            message_len = message.len(),
            "Sending message to user..."
        );

        let connections = self.connections.read().await;
        let lock_time = start.elapsed();

        let Some(senders) = connections.get(user_id) else {
            debug!(
                user_id = %user_id,
                lock_ms = lock_time.as_millis() as u64,
                "No connections found for user"
            );
            return 0;
        };

        let total_connections = senders.len();
        let mut success_count = 0;
        let mut failed_count = 0;

        for (i, sender) in senders.iter().enumerate() {
            match sender.send(message.to_string()) {
                Ok(_) => {
                    success_count += 1;
                    trace!(
                        user_id = %user_id,
                        connection_index = i + 1,
                        total_connections = total_connections,
                        "Message queued successfully"
                    );
                }
                Err(e) => {
                    failed_count += 1;
                    warn!(
                        user_id = %user_id,
                        connection_index = i + 1,
                        error = %e,
                        "Failed to queue message (connection may be dead)"
                    );
                }
            }
        }

        let send_time = start.elapsed();

        debug!(
            user_id = %user_id,
            success = success_count,
            failed = failed_count,
            total_connections = total_connections,
            duration_ms = send_time.as_millis() as u64,
            "Message send complete"
        );

        if failed_count > 0 {
            warn!(
                user_id = %user_id,
                failed_count = failed_count,
                "Some connections failed - may have dead connections to clean up"
            );
        }

        success_count
    }

    /// Get list of all connected user IDs
    pub async fn connected_users(&self) -> Vec<Uuid> {
        let connections = self.connections.read().await;
        let users: Vec<Uuid> = connections.keys().cloned().collect();
        trace!(
            user_count = users.len(),
            "Retrieved connected users list"
        );
        users
    }

    /// Get total number of active connections
    pub async fn total_connections(&self) -> usize {
        let connections = self.connections.read().await;
        let total: usize = connections.values().map(|v| v.len()).sum();
        trace!(
            total_connections = total,
            total_users = connections.len(),
            "Retrieved total connection count"
        );
        total
    }

    /// Get connection statistics (for debugging)
    pub async fn get_stats(&self) -> ConnectionStats {
        let connections = self.connections.read().await;
        let total_users = connections.len();
        let total_connections: usize = connections.values().map(|v| v.len()).sum();
        let max_connections_per_user = connections.values().map(|v| v.len()).max().unwrap_or(0);

        ConnectionStats {
            total_users,
            total_connections,
            max_connections_per_user,
        }
    }
}

/// Connection statistics for debugging
#[derive(Debug)]
pub struct ConnectionStats {
    pub total_users: usize,
    pub total_connections: usize,
    pub max_connections_per_user: usize,
}
