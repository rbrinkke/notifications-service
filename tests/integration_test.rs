use sqlx::PgPool;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;
use chrono::{Utc, Duration as ChronoDuration};

const DB_URL: &str = "postgres://postgres:postgres_secure_password_change_in_prod@localhost:5441/activitydb";

async fn get_pool() -> PgPool {
    PgPool::connect(DB_URL).await.expect("Failed to connect to test DB")
}

async fn wait_for_processed(pool: &PgPool, id: Uuid, timeout_secs: u64) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < timeout_secs {
        let row: (bool,) = sqlx::query_as("SELECT is_processed FROM activity.notifications WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap_or((false,));
        
        if row.0 {
            return true;
        }
        sleep(Duration::from_millis(500)).await;
    }
    false
}

#[tokio::test]
async fn test_instant_notification_delivery() {
    let pool = get_pool().await;
    let id = Uuid::new_v4();
    let user_id = Uuid::new_v4();

    // 1. Insert instant notification
    sqlx::query(
        "INSERT INTO activity.notifications (id, user_id, title, message, notification_type, priority) 
         VALUES ($1, $2, $3, $4, $5, $6)"
    )
    .bind(id)
    .bind(user_id)
    .bind("Rust E2E Test")
    .bind("Testing instant delivery")
    .bind("test")
    .bind("high")
    .execute(&pool)
    .await
    .expect("Failed to insert test notification");

    // 2. Assert it gets processed quickly (via NOTIFY trigger)
    let processed = wait_for_processed(&pool, id, 10).await;
    assert!(processed, "Notification was not processed within timeout");
}

#[tokio::test]
async fn test_scheduled_notification_delivery() {
    let pool = get_pool().await;
    let id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    
    // Schedule 5 seconds in the future
    let deliver_at = Utc::now() + ChronoDuration::seconds(5);

    // 1. Insert scheduled notification
    sqlx::query(
        "INSERT INTO activity.notifications (id, user_id, title, message, notification_type, deliver_at) 
         VALUES ($1, $2, $3, $4, $5, $6)"
    )
    .bind(id)
    .bind(user_id)
    .bind("Rust Scheduled Test")
    .bind("Testing delayed delivery")
    .bind("test")
    .bind(deliver_at)
    .execute(&pool)
    .await
    .expect("Failed to insert scheduled notification");

    // 2. Verify it is NOT processed immediately
    let row: (bool,) = sqlx::query_as("SELECT is_processed FROM activity.notifications WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("Failed to fetch notification status");
    
    assert!(!row.0, "Notification was processed too early!");

    // 3. Wait for delivery time
    sleep(Duration::from_secs(7)).await;

    // 4. Verify it is now processed
    let processed = wait_for_processed(&pool, id, 5).await;
    assert!(processed, "Scheduled notification was not processed after delay");
}

#[tokio::test]
async fn test_broadcast_notification() {
    let pool = get_pool().await;
    let id = Uuid::new_v4();
    let nil_uuid = Uuid::nil();

    // 1. Insert broadcast
    sqlx::query(
        "INSERT INTO activity.notifications (id, user_id, title, message, notification_type) 
         VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(id)
    .bind(nil_uuid)
    .bind("Rust Broadcast Test")
    .bind("Testing global reach")
    .bind("system")
    .execute(&pool)
    .await
    .expect("Failed to insert broadcast notification");

    // 2. Assert it gets processed
    let processed = wait_for_processed(&pool, id, 10).await;
    assert!(processed, "Broadcast notification was not processed");
}
