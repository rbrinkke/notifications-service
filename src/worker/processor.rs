use bus_client::{BusClient, BusEnvelope};
use crate::config::Config;
use crate::db::{NotificationQueries, Database};
use crate::models::Notification;
use crate::push::{FcmClient, fcm::FcmError};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn, instrument};
use uuid::Uuid;

pub struct NotificationWorker {
    pool: PgPool,
    config: Config,
    bus_client: Option<Arc<BusClient>>,
    fcm_client: Option<Arc<FcmClient>>,
}

/// Batch processing statistics
struct BatchStats {
    total: usize,
    bus_success: usize,
    push_success: usize,
    failed: usize,
    duration: Duration,
}

impl NotificationWorker {
    pub fn new(
        db: &Database,
        config: Config,
        bus_client: Option<Arc<BusClient>>,
        fcm_client: Option<Arc<FcmClient>>,
    ) -> Self {
        debug!(
            poll_interval = config.worker_poll_interval_secs,
            batch_size = config.worker_batch_size,
            max_retries = config.max_retries,
            bus_enabled = bus_client.is_some(),
            fcm_enabled = fcm_client.is_some(),
            "Creating NotificationWorker"
        );
        Self {
            pool: db.pool().clone(),
            config,
            bus_client,
            fcm_client,
        }
    }

    /// Main worker loop - wakes on NOTIFY or timeout
    #[instrument(skip(self, wake_rx), name = "worker_loop")]
    pub async fn run(&self, mut wake_rx: mpsc::Receiver<()>) {
        info!("═══════════════════════════════════════════════════════════");
        info!("  NOTIFICATION WORKER STARTED");
        info!("  Poll interval: {}s", self.config.worker_poll_interval_secs);
        info!("  Batch size: {}", self.config.worker_batch_size);
        info!("  Max retries: {}", self.config.max_retries);
        info!("  WebSocket Bus: {}", if self.bus_client.is_some() { "ENABLED" } else { "DISABLED" });
        info!("  FCM: {}", if self.fcm_client.is_some() { "ENABLED" } else { "DISABLED" });
        info!("═══════════════════════════════════════════════════════════");

        let mut cycle_count: u64 = 0;

        loop {
            cycle_count += 1;
            trace!("───────────────────────────────────────────────────────────");
            trace!("Worker cycle #{} starting", cycle_count);

            // Process all pending notifications
            let batch_start = Instant::now();
            self.process_all_pending().await;
            let batch_duration = batch_start.elapsed();

            trace!(
                cycle = cycle_count,
                processing_duration_ms = batch_duration.as_millis() as u64,
                "Worker cycle complete, sleeping..."
            );

            // Sleep until triggered or timeout
            debug!(
                timeout_secs = self.config.worker_poll_interval_secs,
                "Worker sleeping until NOTIFY or timeout"
            );

            let sleep_start = Instant::now();
            tokio::select! {
                // Wake on NOTIFY signal
                Some(_) = wake_rx.recv() => {
                    let sleep_duration = sleep_start.elapsed();
                    debug!(
                        slept_ms = sleep_duration.as_millis() as u64,
                        "Worker WOKE: NOTIFY signal received"
                    );
                    trace!("Wake source: PostgreSQL NOTIFY trigger");
                }
                // Wake on timeout (failsafe)
                _ = tokio::time::sleep(Duration::from_secs(self.config.worker_poll_interval_secs)) => {
                    debug!(
                        timeout_secs = self.config.worker_poll_interval_secs,
                        "Worker WOKE: timeout reached (failsafe poll)"
                    );
                    trace!("Wake source: scheduled timeout");
                }
            }
        }
    }

    /// Process all pending notifications in batches
    #[instrument(skip(self), name = "process_all_pending")]
    async fn process_all_pending(&self) {
        let mut total_processed = 0;
        let mut total_bus = 0;
        let mut total_push = 0;
        let mut total_failed = 0;
        let overall_start = Instant::now();

        loop {
            let fetch_start = Instant::now();
            match NotificationQueries::fetch_unprocessed(&self.pool, self.config.worker_batch_size).await {
                Ok(notifications) if notifications.is_empty() => {
                    if total_processed == 0 {
                        trace!("No pending notifications in queue");
                    }
                    break;
                }
                Ok(notifications) => {
                    let batch_size = notifications.len();
                    let fetch_duration = fetch_start.elapsed();

                    info!("═══ PROCESSING BATCH ═══");
                    info!("  Notifications: {}", batch_size);
                    info!("  Fetch duration: {}ms", fetch_duration.as_millis());

                    trace!("Batch notification IDs:");
                    for n in &notifications {
                        trace!("  - {} (user: {}, type: {})",
                            n.notification_id, n.user_id, n.notification_type);
                    }

                    let batch_start = Instant::now();
                    for (i, notification) in notifications.iter().enumerate() {
                        trace!("Processing {}/{} in batch", i + 1, batch_size);
                        let result = self.process_one(notification.clone()).await;

                        match result {
                            DeliveryResult::Bus => total_bus += 1,
                            DeliveryResult::Push => total_push += 1,
                            DeliveryResult::Failed => total_failed += 1,
                        }
                        total_processed += 1;
                    }

                    let batch_duration = batch_start.elapsed();
                    debug!(
                        batch_size = batch_size,
                        duration_ms = batch_duration.as_millis() as u64,
                        avg_ms = if batch_size > 0 { batch_duration.as_millis() as u64 / batch_size as u64 } else { 0 },
                        "Batch processed"
                    );
                }
                Err(e) => {
                    error!(
                        error = %e,
                        duration_ms = fetch_start.elapsed().as_millis() as u64,
                        "Failed to fetch notifications from database"
                    );
                    break;
                }
            }
        }

        // Log batch summary if anything was processed
        if total_processed > 0 {
            let overall_duration = overall_start.elapsed();
            info!("═══════════════════════════════════════════════════════════");
            info!("  BATCH COMPLETE");
            info!("  Total processed: {}", total_processed);
            info!("  Success via Bus: {}", total_bus);
            info!("  Success via Push: {}", total_push);
            info!("  Failed (will retry): {}", total_failed);
            info!("  Total duration: {}ms", overall_duration.as_millis());
            info!("  Avg per notification: {}ms",
                if total_processed > 0 { overall_duration.as_millis() / total_processed as u128 } else { 0 });
            info!("═══════════════════════════════════════════════════════════");
        }
    }

    /// Process a single notification
    #[instrument(skip(self, notification), fields(
        notification_id = %notification.notification_id,
        user_id = %notification.user_id,
        notification_type = %notification.notification_type
    ))]
    async fn process_one(&self, notification: Notification) -> DeliveryResult {
        let notification_id = notification.notification_id;
        let user_id = notification.user_id;
        let start = Instant::now();

        trace!("══════════════════════════════════════════════════");
        trace!("PROCESSING NOTIFICATION");
        trace!("  notification_id: {}", notification_id);
        trace!("  user_id: {}", user_id);
        trace!("  type: {}", notification.notification_type);
        trace!("  title: {:?}", notification.title);
        trace!("  message: {:?}", notification.message);
        trace!("  priority: {:?}", notification.priority);
        trace!("  created_at: {}", notification.created_at);
        trace!("══════════════════════════════════════════════════");

        // Try WebSocket Bus first if configured
        if let Some(bus) = &self.bus_client {
            trace!("Attempting delivery via WebSocket Bus...");

            match self.send_via_bus(bus, &notification).await {
                Ok(delivered_to) if delivered_to > 0 => {
                    let duration = start.elapsed();
                    info!(
                        notification_id = %notification_id,
                        user_id = %user_id,
                        delivered_to = delivered_to,
                        duration_ms = duration.as_millis() as u64,
                        "✓ Delivered via WebSocket Bus"
                    );
                    self.mark_success(notification_id).await;
                    return DeliveryResult::Bus;
                }
                Ok(_) => {
                    // delivered_to == 0: User has no active connections
                    debug!(
                        user_id = %user_id,
                        "User has no active WebSocket connections, falling back to FCM"
                    );
                }
                Err(e) => {
                    warn!(
                        notification_id = %notification_id,
                        user_id = %user_id,
                        error = %e,
                        "WebSocket Bus delivery failed, falling back to FCM"
                    );
                }
            }
        } else {
            debug!(
                user_id = %user_id,
                "WebSocket Bus not configured, trying FCM directly"
            );
        }

        // User offline or Bus failed/not configured - try push notification
        trace!("Attempting push notification delivery...");
        match self.send_via_push(&notification).await {
            Ok(device_count) => {
                let duration = start.elapsed();
                info!(
                    notification_id = %notification_id,
                    user_id = %user_id,
                    devices = device_count,
                    duration_ms = duration.as_millis() as u64,
                    "✓ Delivered via Push"
                );
                self.mark_success(notification_id).await;
                DeliveryResult::Push
            }
            Err(e) => {
                let duration = start.elapsed();
                warn!(
                    notification_id = %notification_id,
                    user_id = %user_id,
                    error = %e,
                    duration_ms = duration.as_millis() as u64,
                    "✗ Delivery failed"
                );
                self.mark_failure(notification_id, &e).await;
                DeliveryResult::Failed
            }
        }
    }

    /// Send sync_notify via WebSocket Bus
    #[instrument(skip(self, bus, notification), fields(
        notification_id = %notification.notification_id,
        user_id = %notification.user_id
    ))]
    async fn send_via_bus(&self, bus: &BusClient, notification: &Notification) -> Result<usize, String> {
        let start = Instant::now();

        // Create sync_notify envelope
        // This tells the client to fetch new notifications from the API
        let envelope = BusEnvelope::new("notifications", "sync_notify")
            .with_payload(serde_json::json!({
                "type": "sync_notify",
                "count": 1
            }));

        trace!("sync_notify envelope created: {:?}", envelope);
        trace!("Publishing to user {} via WebSocket Bus...", notification.user_id);

        match bus.publish_to_user(notification.user_id, &envelope).await {
            Ok(response) => {
                let duration = start.elapsed();
                debug!(
                    notification_id = %notification.notification_id,
                    user_id = %notification.user_id,
                    delivered_to = response.delivered_to,
                    duration_ms = duration.as_millis() as u64,
                    "sync_notify published via Bus"
                );
                Ok(response.delivered_to)
            }
            Err(e) => {
                let duration = start.elapsed();
                warn!(
                    user_id = %notification.user_id,
                    error = %e,
                    duration_ms = duration.as_millis() as u64,
                    "Failed to publish to WebSocket Bus"
                );
                Err(e.to_string())
            }
        }
    }

    /// Send push notification via FCM
    #[instrument(skip(self, notification), fields(
        notification_id = %notification.notification_id,
        user_id = %notification.user_id
    ))]
    async fn send_via_push(&self, notification: &Notification) -> Result<usize, String> {
        let start = Instant::now();

        let Some(fcm) = &self.fcm_client else {
            debug!("FCM client not configured, cannot send push");
            return Err("FCM not configured".to_string());
        };

        // Get user's devices
        trace!("Fetching FCM devices for user {}", notification.user_id);
        let devices = NotificationQueries::get_user_devices(&self.pool, notification.user_id)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to fetch user devices from database");
                format!("Failed to get devices: {}", e)
            })?;

        if devices.is_empty() {
            debug!(
                user_id = %notification.user_id,
                "No registered FCM devices for user"
            );
            return Err("No registered devices".to_string());
        }

        trace!(
            device_count = devices.len(),
            "Found {} FCM devices, sending push to each",
            devices.len()
        );

        let mut success_count = 0;
        let mut invalid_count = 0;
        let mut error_count = 0;
        let mut last_error = None;

        for (i, device) in devices.iter().enumerate() {
            let device_start = Instant::now();
            let token_preview = mask_token(&device.fcm_token);

            trace!(
                device_index = i + 1,
                device_type = %device.device_type,
                token = %token_preview,
                "Sending FCM push to device {}/{}",
                i + 1,
                devices.len()
            );

            match fcm.send(&device.fcm_token, notification).await {
                Ok(()) => {
                    let device_duration = device_start.elapsed();
                    debug!(
                        device_index = i + 1,
                        device_type = %device.device_type,
                        token = %token_preview,
                        duration_ms = device_duration.as_millis() as u64,
                        "✓ FCM push sent successfully"
                    );
                    success_count += 1;
                }
                Err(FcmError::InvalidToken) => {
                    warn!(
                        device_type = %device.device_type,
                        token = %token_preview,
                        "✗ Invalid FCM token, removing from database"
                    );
                    invalid_count += 1;
                    if let Err(e) = NotificationQueries::remove_device(&self.pool, &device.fcm_token).await {
                        error!(error = %e, "Failed to remove invalid FCM token");
                    }
                }
                Err(e) => {
                    let device_duration = device_start.elapsed();
                    error!(
                        device_type = %device.device_type,
                        token = %token_preview,
                        error = %e,
                        duration_ms = device_duration.as_millis() as u64,
                        "✗ FCM push failed"
                    );
                    error_count += 1;
                    last_error = Some(e.to_string());
                }
            }
        }

        let total_duration = start.elapsed();

        debug!(
            total_devices = devices.len(),
            success = success_count,
            invalid_tokens = invalid_count,
            errors = error_count,
            duration_ms = total_duration.as_millis() as u64,
            "FCM push batch complete"
        );

        if success_count > 0 {
            Ok(success_count)
        } else {
            Err(last_error.unwrap_or_else(|| "All push attempts failed".to_string()))
        }
    }

    /// Mark notification as successfully delivered
    #[instrument(skip(self), fields(notification_id = %notification_id))]
    async fn mark_success(&self, notification_id: Uuid) {
        trace!("Marking notification {} as success", notification_id);
        let start = Instant::now();

        if let Err(e) = NotificationQueries::mark_success(&self.pool, notification_id).await {
            error!(
                notification_id = %notification_id,
                error = %e,
                duration_ms = start.elapsed().as_millis() as u64,
                "Failed to mark notification as success in database"
            );
        } else {
            trace!(
                notification_id = %notification_id,
                duration_ms = start.elapsed().as_millis() as u64,
                "Notification marked as processed"
            );
        }
    }

    /// Mark notification failure with error tracking
    #[instrument(skip(self), fields(notification_id = %notification_id, error = %error))]
    async fn mark_failure(&self, notification_id: Uuid, error: &str) {
        trace!(
            "Recording failure for notification {}: {}",
            notification_id, error
        );
        let start = Instant::now();

        match NotificationQueries::mark_failure(
            &self.pool,
            notification_id,
            error,
            self.config.max_retries,
        ).await {
            Ok(stopped) => {
                let duration = start.elapsed();
                if stopped {
                    warn!(
                        notification_id = %notification_id,
                        max_retries = self.config.max_retries,
                        duration_ms = duration.as_millis() as u64,
                        "Notification permanently failed - max retries reached"
                    );
                } else {
                    debug!(
                        notification_id = %notification_id,
                        error = %error,
                        duration_ms = duration.as_millis() as u64,
                        "Notification failure recorded, will retry later"
                    );
                }
            }
            Err(e) => {
                error!(
                    notification_id = %notification_id,
                    error = %e,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "Failed to record notification failure in database"
                );
            }
        }
    }
}

/// Result of notification delivery attempt
enum DeliveryResult {
    Bus,
    Push,
    Failed,
}

/// Mask FCM token for logging (security)
fn mask_token(token: &str) -> String {
    if token.len() > 12 {
        format!("{}...{}", &token[..6], &token[token.len()-4..])
    } else if token.len() > 4 {
        format!("{}...", &token[..4])
    } else {
        "****".to_string()
    }
}
