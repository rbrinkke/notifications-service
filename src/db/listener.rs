use sqlx::postgres::PgListener;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

const NOTIFY_CHANNEL: &str = "notify_event";

pub struct NotificationListener {
    database_url: String,
}

impl NotificationListener {
    pub fn new(database_url: String) -> Self {
        debug!("Creating NotificationListener for channel '{}'", NOTIFY_CHANNEL);
        Self { database_url }
    }

    /// Start listening for NOTIFY events and send signals to the worker
    pub async fn listen(&self, tx: mpsc::Sender<()>) -> Result<(), sqlx::Error> {
        info!("═══════════════════════════════════════════════════════════");
        info!("  NOTIFY LISTENER STARTING");
        info!("  Channel: {}", NOTIFY_CHANNEL);
        info!("═══════════════════════════════════════════════════════════");

        let mut reconnect_count = 0;

        loop {
            reconnect_count += 1;
            if reconnect_count > 1 {
                debug!(
                    attempt = reconnect_count,
                    "Reconnecting to PostgreSQL NOTIFY..."
                );
            }

            match self.listen_loop(&tx, reconnect_count).await {
                Ok(_) => {
                    warn!(
                        reconnect_count = reconnect_count,
                        "Listener loop ended unexpectedly (no error), restarting..."
                    );
                }
                Err(e) => {
                    error!(
                        error = %e,
                        reconnect_count = reconnect_count,
                        "Listener error, reconnecting in 5s..."
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn listen_loop(&self, tx: &mpsc::Sender<()>, session_id: u64) -> Result<(), sqlx::Error> {
        trace!("Connecting to PostgreSQL for LISTEN...");
        let connect_start = Instant::now();

        let mut listener = PgListener::connect(&self.database_url).await?;

        debug!(
            duration_ms = connect_start.elapsed().as_millis() as u64,
            "PostgreSQL connection established for LISTEN"
        );

        trace!("Subscribing to channel '{}'...", NOTIFY_CHANNEL);
        listener.listen(NOTIFY_CHANNEL).await?;

        info!(
            channel = NOTIFY_CHANNEL,
            session_id = session_id,
            "✓ Now listening for PostgreSQL NOTIFY events"
        );

        let mut message_count: u64 = 0;

        loop {
            trace!("Waiting for next NOTIFY event...");
            let wait_start = Instant::now();

            match listener.recv().await {
                Ok(notification) => {
                    message_count += 1;
                    let wait_duration = wait_start.elapsed();

                    debug!(
                        message_number = message_count,
                        session_id = session_id,
                        channel = notification.channel(),
                        payload = notification.payload(),
                        wait_duration_ms = wait_duration.as_millis() as u64,
                        "NOTIFY received"
                    );

                    trace!(
                        "NOTIFY details: channel='{}', payload='{}', raw_len={}",
                        notification.channel(),
                        notification.payload(),
                        notification.payload().len()
                    );

                    // Signal worker to wake up
                    trace!("Sending wake signal to worker...");
                    match tx.try_send(()) {
                        Ok(_) => {
                            debug!(
                                message_number = message_count,
                                "Wake signal sent to worker successfully"
                            );
                        }
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            warn!(
                                message_number = message_count,
                                queue_capacity = tx.capacity(),
                                "Wake signal channel FULL - worker is busy (will process on next cycle)"
                            );
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            error!(
                                message_number = message_count,
                                "Wake signal channel CLOSED - worker may have crashed!"
                            );
                            // Continue anyway, maybe it will be fixed
                        }
                    }
                }
                Err(e) => {
                    error!(
                        error = %e,
                        message_count = message_count,
                        session_id = session_id,
                        "Error receiving NOTIFY event"
                    );
                    return Err(e);
                }
            }
        }
    }
}
