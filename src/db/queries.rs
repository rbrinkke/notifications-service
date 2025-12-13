use crate::models::Notification;
use sqlx::PgPool;
use std::time::Instant;
use tracing::{debug, error, info, trace, warn, instrument};
use uuid::Uuid;

pub struct NotificationQueries;

impl NotificationQueries {
    /// Fetch all unprocessed notifications
    #[instrument(skip(pool), fields(limit = limit))]
    pub async fn fetch_unprocessed(
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<Notification>, sqlx::Error> {
        trace!("DB fetch_unprocessed: starting query with limit={}", limit);
        let start = Instant::now();

        let result = sqlx::query_as::<_, Notification>(
            r#"
            SELECT
                notification_id,
                user_id,
                actor_user_id,
                notification_type::text as notification_type,
                target_type,
                target_id,
                title,
                message,
                payload,
                deep_link,
                priority,
                created_at
            FROM activity.notifications
            WHERE is_processed = false
            ORDER BY created_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(pool)
        .await;

        let duration = start.elapsed();

        match &result {
            Ok(notifications) => {
                let count = notifications.len();
                debug!(
                    duration_ms = duration.as_millis() as u64,
                    count = count,
                    "DB fetch_unprocessed: completed"
                );

                if count > 0 {
                    trace!("DB fetch_unprocessed: notification IDs:");
                    for n in notifications.iter() {
                        trace!(
                            "  - {} (user={}, type={}, created={})",
                            n.notification_id,
                            n.user_id,
                            n.notification_type,
                            n.created_at
                        );
                    }
                } else {
                    trace!("DB fetch_unprocessed: no pending notifications");
                }
            }
            Err(e) => {
                error!(
                    duration_ms = duration.as_millis() as u64,
                    error = %e,
                    "DB fetch_unprocessed: query failed"
                );
            }
        }

        result
    }

    /// Mark notification as successfully delivered
    #[instrument(skip(pool), fields(notification_id = %notification_id))]
    pub async fn mark_success(
        pool: &PgPool,
        notification_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        trace!("DB mark_success: calling sp_notification_success({})", notification_id);
        let start = Instant::now();

        let result = sqlx::query_as::<_, (bool,)>(
            "SELECT activity.sp_notification_success($1)"
        )
        .bind(notification_id)
        .fetch_one(pool)
        .await;

        let duration = start.elapsed();

        match &result {
            Ok((success,)) => {
                if *success {
                    debug!(
                        notification_id = %notification_id,
                        duration_ms = duration.as_millis() as u64,
                        "DB mark_success: notification marked as processed"
                    );
                } else {
                    warn!(
                        notification_id = %notification_id,
                        duration_ms = duration.as_millis() as u64,
                        "DB mark_success: stored procedure returned false (notification not found?)"
                    );
                }
            }
            Err(e) => {
                error!(
                    notification_id = %notification_id,
                    duration_ms = duration.as_millis() as u64,
                    error = %e,
                    "DB mark_success: failed to mark notification"
                );
            }
        }

        result.map(|(success,)| success)
    }

    /// Record delivery failure - returns true if max retries reached (stop trying)
    #[instrument(skip(pool), fields(notification_id = %notification_id, error_message = %error_message, max_retries = max_retries))]
    pub async fn mark_failure(
        pool: &PgPool,
        notification_id: Uuid,
        error_message: &str,
        max_retries: i32,
    ) -> Result<bool, sqlx::Error> {
        trace!(
            "DB mark_failure: calling sp_notification_failure({}, '{}', {})",
            notification_id, error_message, max_retries
        );
        let start = Instant::now();

        let result = sqlx::query_as::<_, (bool,)>(
            "SELECT activity.sp_notification_failure($1, $2, $3)"
        )
        .bind(notification_id)
        .bind(error_message)
        .bind(max_retries)
        .fetch_one(pool)
        .await;

        let duration = start.elapsed();

        match &result {
            Ok((max_reached,)) => {
                if *max_reached {
                    warn!(
                        notification_id = %notification_id,
                        duration_ms = duration.as_millis() as u64,
                        max_retries = max_retries,
                        error_message = %error_message,
                        "DB mark_failure: MAX RETRIES REACHED - notification will not be retried"
                    );
                } else {
                    debug!(
                        notification_id = %notification_id,
                        duration_ms = duration.as_millis() as u64,
                        error_message = %error_message,
                        "DB mark_failure: error recorded, will retry later"
                    );
                }
            }
            Err(e) => {
                error!(
                    notification_id = %notification_id,
                    duration_ms = duration.as_millis() as u64,
                    error = %e,
                    "DB mark_failure: failed to record failure"
                );
            }
        }

        result.map(|(max_reached,)| max_reached)
    }

    /// Get FCM tokens for a user
    #[instrument(skip(pool), fields(user_id = %user_id))]
    pub async fn get_user_devices(
        pool: &PgPool,
        user_id: Uuid,
    ) -> Result<Vec<UserDevice>, sqlx::Error> {
        trace!("DB get_user_devices: fetching devices for user {}", user_id);
        let start = Instant::now();

        let result = sqlx::query_as::<_, UserDevice>(
            r#"
            SELECT fcm_token, device_type
            FROM activity.user_devices
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_all(pool)
        .await;

        let duration = start.elapsed();

        match &result {
            Ok(devices) => {
                let count = devices.len();
                debug!(
                    user_id = %user_id,
                    device_count = count,
                    duration_ms = duration.as_millis() as u64,
                    "DB get_user_devices: completed"
                );

                if count > 0 {
                    trace!("DB get_user_devices: device types:");
                    for (i, device) in devices.iter().enumerate() {
                        // Only show first 8 chars of token for security
                        let token_preview = if device.fcm_token.len() > 8 {
                            format!("{}...", &device.fcm_token[..8])
                        } else {
                            device.fcm_token.clone()
                        };
                        trace!(
                            "  Device {}: type={}, token={}",
                            i + 1,
                            device.device_type,
                            token_preview
                        );
                    }
                } else {
                    debug!(
                        user_id = %user_id,
                        "DB get_user_devices: user has no registered devices"
                    );
                }
            }
            Err(e) => {
                error!(
                    user_id = %user_id,
                    duration_ms = duration.as_millis() as u64,
                    error = %e,
                    "DB get_user_devices: query failed"
                );
            }
        }

        result
    }

    /// Remove invalid FCM token
    #[instrument(skip(pool, fcm_token), fields(token_preview = %Self::mask_token(fcm_token)))]
    pub async fn remove_device(pool: &PgPool, fcm_token: &str) -> Result<(), sqlx::Error> {
        let token_preview = Self::mask_token(fcm_token);
        trace!("DB remove_device: deleting device with token {}", token_preview);
        let start = Instant::now();

        let result = sqlx::query("DELETE FROM activity.user_devices WHERE fcm_token = $1")
            .bind(fcm_token)
            .execute(pool)
            .await;

        let duration = start.elapsed();

        match &result {
            Ok(query_result) => {
                let rows_affected = query_result.rows_affected();
                if rows_affected > 0 {
                    info!(
                        token_preview = %token_preview,
                        rows_affected = rows_affected,
                        duration_ms = duration.as_millis() as u64,
                        "DB remove_device: invalid FCM token removed"
                    );
                } else {
                    debug!(
                        token_preview = %token_preview,
                        duration_ms = duration.as_millis() as u64,
                        "DB remove_device: token not found (already removed?)"
                    );
                }
            }
            Err(e) => {
                error!(
                    token_preview = %token_preview,
                    duration_ms = duration.as_millis() as u64,
                    error = %e,
                    "DB remove_device: failed to remove token"
                );
            }
        }

        result.map(|_| ())
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
}

#[derive(Debug, sqlx::FromRow)]
pub struct UserDevice {
    pub fcm_token: String,
    pub device_type: String,
}
