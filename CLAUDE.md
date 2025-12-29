# Notifications Service

Background worker for notification delivery (NOT an API). Dual delivery: WebSocket Bus → FCM push fallback.

## Commands

```bash
cargo build && cargo run     # Dev on :8080 (health only)
cargo test
docker build --no-cache -t notifications-service . && k3d image import notifications-service -c activity-local
kubectl rollout restart deployment/notifications-service -n activity-prod
```

## Dev Config

```
Port: 8080 (health endpoint only)
DATABASE_URL=postgresql://postgres:...@localhost:5441/activitydb
WEBSOCKET_BUS_URL=http://bus.localhost:8080
SERVICE_TOKEN=dev-token
GOOGLE_APPLICATION_CREDENTIALS=/path/to/firebase-key.json
FCM_PROJECT_ID=your-project-id
```

## Architecture

```
PostgreSQL NOTIFY (new notification)
     ↓
Worker picks up
     ↓
Try WebSocket Bus delivery
     ↓ (if user offline)
FCM Push notification
     ↓
Mark as delivered in DB
```

## Critical Gotchas

1. **Broadcast (user_id=nil) ALWAYS marked success** - never blocks queue
2. **NOTIFY buffer = 10** - extra signals dropped if worker busy
3. **Invalid FCM tokens auto-removed** from database
4. **DEBUG_MODE=true logs entire FCM tokens** - SECURITY RISK in prod

## Health Check

```bash
curl http://localhost:8080/health
```

Only endpoint - this is a worker, not an API.
