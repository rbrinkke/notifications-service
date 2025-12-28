# Notifications Service

**Status:** Production Ready 🚀  
**Tech Stack:** Rust (Tokio, Axum, SQLx) | PostgreSQL | WebSocket Bus | Firebase (FCM)

The **Notifications Service** is a high-performance, event-driven microservice responsible for delivering notifications to users across the GoAmet platform. It implements a "Best in Class" architecture prioritizing real-time delivery via WebSockets, with a robust fallback to Push Notifications (FCM) for offline users.

## ✨ Key Features

*   **⚡ Ultra-Low Latency:** Uses PostgreSQL `NOTIFY` triggers to wake up the Rust worker immediately upon data insertion. Average processing time < 2ms.
*   **🚌 Unified WebSocket Bus:** Integrates with the internal `websocket-bus` via HTTP to deliver real-time `sync_notify` signals to active clients.
*   **📱 Smart Fallback Strategy:**
    1.  Attempts delivery via **WebSocket Bus** (if user is online).
    2.  Falls back to **FCM (Firebase Cloud Messaging)** if the user is offline or unreachable via Bus.
*   **📢 Global Broadcasts:** Support for platform-wide announcements using a special Nil UUID (`0000...`).
    *   Sends to `global_notifications` topic on the Bus.
    *   Sends to `all` topic on FCM.
*   **⏰ Scheduled Delivery:** Intelligent scheduling using the `deliver_at` column. Notifications can be created now but delivered in the future.
*   **🛡️ Robust Error Handling:** Automatic retries, error tracking (`last_error`), and exponential backoff strategies.

---

## 🏗️ Architecture

### Data Flow
1.  **Ingest:** Any service inserts a row into `activity.notifications`.
2.  **Trigger:** Postgres fires a `pg_notify` event on the `notify_event` channel.
3.  **Wake:** The Rust worker (listening on the DB connection) wakes up instantly.
4.  **Process:**
    *   Worker fetches batch of `unprocessed` notifications where `deliver_at <= NOW()`.
    *   **If User Specific:** Tries Bus -> Tries FCM.
    *   **If Broadcast:** Publishes to Bus Topic & FCM Topic.
5.  **Complete:** Updates `is_processed = true` or increments `error_count`.

---

## 💾 Database Schema

The service operates on the `activity.notifications` table.

| Column | Type | Description |
| :--- | :--- | :--- |
| `id` | UUID | Unique Identifier (Primary Key) |
| `user_id` | UUID | Recipient UUID. Use `00000000-0000-0000-0000-000000000000` for Broadcasts. |
| `title` | TEXT | Notification title. |
| `message` | TEXT | Body text. |
| `notification_type` | TEXT | E.g., `system`, `friend_request`, `comment`. |
| `is_processed` | BOOLEAN | `false` = pending, `true` = delivered/done. |
| `deliver_at` | TIMESTAMPTZ | **New:** When to send. Defaults to `now()`. |
| `priority` | TEXT | `normal`, `high`, `critical`. |
| `payload` | JSONB | Extra data (deep links, metadata). |

---

## 🚀 Usage Guide

### 1. Sending a Standard Notification
Insert a record. It will be delivered immediately.

```sql
INSERT INTO activity.notifications 
(user_id, title, message, notification_type, priority)
VALUES 
('user-uuid-here', 'Hello!', 'You have a new message', 'chat_message', 'high');
```

### 2. Sending a Scheduled Notification ⏰
Set `deliver_at` to a future timestamp. The worker will pick it up automatically when the time comes.

```sql
INSERT INTO activity.notifications 
(user_id, title, message, notification_type, deliver_at)
VALUES 
('user-uuid-here', 'Reminder', 'Event starts in 10 mins', 'reminder', NOW() + INTERVAL '10 minutes');
```

### 3. Sending a Global Broadcast 📢
Use the Nil UUID. This sends to **ALL** users via Bus and FCM topics.

```sql
INSERT INTO activity.notifications 
(user_id, title, message, notification_type)
VALUES 
('00000000-0000-0000-0000-000000000000', 'System Alert', 'Maintenance in 1 hour', 'system');
```

---

## ⚙️ Configuration

Environment variables required for `deployment.yaml` or `.env`.

| Variable | Description | Example |
| :--- | :--- | :--- |
| `DATABASE_URL` | Postgres connection string | `postgres://user:pass@host:5432/db` |
| `WEBSOCKET_BUS_URL` | **HTTP** URL of the Bus Service | `http://websocket-bus:8080` |
| `SERVICE_TOKEN` | Auth token for the Bus | `secret-token` |
| `FCM_PROJECT_ID` | Firebase Project ID | `goamet-notifications` |
| `GOOGLE_APPLICATION_CREDENTIALS` | Path to JSON key | `/secrets/fcm/service-account.json` |
| `WORKER_POLL_INTERVAL_SECS` | Failsafe poll interval | `60` |

---

## 🛠️ Development

### Local Setup
```bash
# 1. Ensure Postgres and Websocket Bus are running
# 2. Configure .env
cp .env.example .env

# 3. Run
cargo run
```

### Deployment (Kubernetes)
The service is built using a highly optimized Dockerfile with `cargo-chef` caching.

```bash
# Build
docker build -t notifications-service:latest .

# Deploy
kubectl apply -f k8s/deployment.yaml
```

---

*Built with elegance and precision.*
