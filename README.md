# Notifications Service (Rust)

High-performance notification delivery service built in Rust.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  notifications-service                                       │
│                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐ │
│  │ WebSocket   │  │ Postgres    │  │ FCM Push            │ │
│  │ Server      │  │ Listener    │  │ Client              │ │
│  │ (axum)      │  │ (NOTIFY)    │  │ (HTTP v1 API)       │ │
│  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘ │
│         │                │                     │            │
│         └────────────────┼─────────────────────┘            │
│                          │                                  │
│                   ┌──────▼──────┐                          │
│                   │ Worker Loop │                          │
│                   │ (select!)   │                          │
│                   └─────────────┘                          │
└─────────────────────────────────────────────────────────────┘
```

## Flow

1. **INSERT** into `activity.notifications` → Postgres NOTIFY
2. **Worker** wakes up (or after 60s timeout)
3. **Fetch** all `is_processed = false`
4. Per notification:
   - If user connected via WebSocket → send `sync_notify`
   - If user offline → send FCM push notification
5. **On SUCCESS** → `is_processed = true`
6. **On FAILURE** → `error_count++`, retry until `max_retries`

## Build

### Docker (recommended)
```bash
docker build -t notifications-service .
```

### Local (requires Rust)
```bash
cargo build --release
```

## Run

### Environment Variables
```bash
cp .env.example .env
# Edit .env with your settings
```

### Docker
```bash
docker run --env-file .env -p 8080:8080 notifications-service
```

### Local
```bash
cargo run --release
```

## Database Migrations

Run these SQL files in order:
```bash
psql -h localhost -p 5441 -U postgres -d activitydb -f migrations/001_error_tracking.sql
psql -h localhost -p 5441 -U postgres -d activitydb -f migrations/002_notify_trigger.sql
```

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /ws?user_id=UUID` | WebSocket connection |
| `GET /health` | Health check |
| `GET /metrics` | Prometheus metrics |

## WebSocket Protocol

### Client → Server
```json
{"type": "ping"}
{"type": "sync_complete", "notification_ids": ["uuid1", "uuid2"]}
```

### Server → Client
```json
{"type": "connected", "user_id": "uuid"}
{"type": "pong"}
{"type": "sync_notify", "count": 5}
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | - | PostgreSQL connection string |
| `WEBSOCKET_PORT` | 8080 | WebSocket server port |
| `WORKER_POLL_INTERVAL_SECS` | 60 | Fallback poll interval |
| `WORKER_BATCH_SIZE` | 100 | Max notifications per batch |
| `MAX_RETRIES` | 3 | Max delivery attempts before giving up |
| `FCM_PROJECT_ID` | - | Firebase project ID |
| `GOOGLE_APPLICATION_CREDENTIALS` | - | Path to FCM service account JSON |

## Error Handling

- **Success**: `is_processed = true`
- **Failure**: `error_count++`, `last_error` logged
- **Max retries reached**: `is_processed = true` (stop trying)

No infinite loops. No lost notifications.
