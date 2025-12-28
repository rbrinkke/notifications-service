# Gap Analyse: Notifications-Service ⟷ Core Integratie

**Datum:** 2025-12-27
**Status:** Analyse compleet

---

## Executive Summary

De notifications-service **publiceert** al correct naar de websocket-bus (topic: `notifications`, event: `sync_notify`). De Rust core (goamet_app) heeft echter **geen consumer** die naar deze events luistert. Dit document beschrijft wat er nog ontbreekt.

---

## 1. Huidige Architectuur

### 1.1 Notifications-Service (SENDER) ✅

**Locatie:** `/opt/goamet/k3s/services/notifications-service/`

**Hoe het werkt:**
```
PostgreSQL NOTIFY trigger
        ↓
NotificationListener → wake channel
        ↓
NotificationWorker.process_one()
        ↓
send_via_bus() → BusClient.publish_to_user()
        ↓
websocket-bus /internal/publish/user/{user_id}
```

**Envelope format (processor.rs:377-381):**
```rust
BusEnvelope::new("notifications", "sync_notify")
    .with_payload(json!({
        "type": "sync_notify",
        "count": 1
    }))
```

**Status:** ✅ Volledig werkend

### 1.2 Websocket-Bus (BROKER) ✅

**Locatie:** `/opt/goamet/k3s/services/websocket-bus/`

**Ondersteunde topics:**
- `chat:{conversation_id}` - Chat berichten
- `notifications` - User notifications ✅
- `wal:{user_id}` - Data sync frames
- `global_notifications` - Broadcasts

**Endpoint:** `POST /internal/publish/user/{user_id}`

**Status:** ✅ Ondersteunt `notifications` topic

### 1.3 Bus-Client Library ✅

**Locatie:** `/opt/goamet/k3s/libs/bus-client/`

**Beschikbare methodes:**
- `publish(envelope)` - Publish naar topic
- `publish_to_user(user_id, envelope)` - Publish naar specifieke user
- `publish_batch(envelopes)` - Batch publish
- `create_ticket(user_id, org_id, topics, ttl)` - Maak WS ticket

**Status:** ✅ Volledig, inclusief `BusEnvelope::notification()` helper

### 1.4 Goamet Core (RECEIVER) ❌

**Locatie:** `/opt/goamet/goamet_app/crates/core/`

**Huidige bus integratie:**
- `runtime/bus.rs` - WebSocket client voor **ACTION EXECUTION**
- Verbindt met `actions` topic voor remote action calls
- **NIET** subscribed op `notifications` topic

**Wat ontbreekt:**
1. Geen subscription op `notifications` topic
2. Geen event handler voor `sync_notify` events
3. Geen invalidatie van notification_cache bij ontvangst

---

## 2. Gap Analyse

### GAP 1: Geen Notification Topic Subscription

**Probleem:** De BusClient in `runtime/bus.rs` subscribed alleen op `actions`:

```rust
// bus.rs:253-254 - CreateTicketRequest
authorized_topics: vec!["actions".to_string()],  // ← ALLEEN actions
```

**Impact:** Clients ontvangen geen real-time notificaties

### GAP 2: Geen Event Handler voor sync_notify

**Probleem:** De message handler in `handle_message()` (bus.rs:436-510) verwerkt alleen `action_result` events:

```rust
let is_action_result = msg_type == Some("action_result")
    || (msg_type == Some("event") && event_type == Some("action_result"));
```

**Nodig:** Handler voor `event_type == "sync_notify"`

### GAP 3: Geen Cache Invalidatie Mechanisme

**Probleem:** De web app leest uit `notification_cache` (SQLite), maar heeft geen mechanisme om deze te invalideren wanneer een `sync_notify` binnenkomt.

**Huidige flow (web routes/notifications.rs):**
```rust
// Query notification_cache (synced from PostgreSQL)
SELECT * FROM notification_cache WHERE user_id = ?
```

**Nodig:**
- Signaal wanneer cache moet worden ge-refreshed
- Of: Direct update van cache bij binnenkomend event

### GAP 4: Geen WebSocket Verbinding vanuit Web UI

**Probleem:** De Rust web app (axum) rendert HTML templates, maar de browser heeft geen WebSocket verbinding met websocket-bus.

**Alternatieven:**
1. **Browser → websocket-bus direct** (via JS)
2. **SSE endpoint in web app** die events doorstuurt
3. **Polling** (minder elegant maar simpel)

---

## 3. Aanbevolen Oplossing

### Architectuur Keuze: Hybride Aanpak

```
┌─────────────────────────────────────────────────────────────────┐
│                        BROWSER                                   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  JavaScript WebSocket Client                              │   │
│  │  - Connect: wss://bus.goamet.nl/api/v1/bus?ticket=...   │   │
│  │  - Subscribe: ["notifications"]                          │   │
│  │  - On sync_notify: htmx.trigger('#notifications', 'refresh')│
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ↓ WS
┌─────────────────────────────────────────────────────────────────┐
│                     WEBSOCKET-BUS                                │
│  - Route notifications naar geauthenticeerde users              │
│  - Ticket validatie met user_id claim                           │
└─────────────────────────────────────────────────────────────────┘
                              ↑ HTTP /internal/publish
┌─────────────────────────────────────────────────────────────────┐
│                  NOTIFICATIONS-SERVICE                           │
│  - Publishes sync_notify naar websocket-bus                     │
│  - ✅ WERKT AL                                                   │
└─────────────────────────────────────────────────────────────────┘
```

### Stappen

#### Stap 1: Ticket Endpoint in Web App

**Nieuw endpoint:** `GET /api/bus-ticket`

```rust
// web/routes/bus_ticket.rs
pub async fn get_bus_ticket(
    Extension(auth_user): Extension<AuthenticatedUser>,
    State(config): State<Config>,
) -> Json<TicketResponse> {
    // Call websocket-bus /internal/create-ticket
    let response = reqwest::Client::new()
        .post(&format!("{}/internal/create-ticket", config.bus_url))
        .header("X-Service-Token", &config.service_token)
        .json(&json!({
            "user_id": auth_user.id,
            "org_id": auth_user.org_id,
            "authorized_topics": ["notifications"],
            "ttl_seconds": 3600
        }))
        .send()
        .await;
    // Return ticket to frontend
}
```

#### Stap 2: JavaScript WebSocket Client

**In templates/base.html:**

```javascript
class NotificationSocket {
    constructor() {
        this.connect();
    }

    async connect() {
        // 1. Get ticket from backend
        const resp = await fetch('/api/bus-ticket');
        const { ticket, ws_url } = await resp.json();

        // 2. Connect to websocket-bus
        this.ws = new WebSocket(`${ws_url}?ticket=${ticket}`);

        this.ws.onopen = () => {
            // Subscribe to notifications
            this.ws.send(JSON.stringify({
                type: "subscribe",
                topics: ["notifications"]
            }));
        };

        this.ws.onmessage = (event) => {
            const msg = JSON.parse(event.data);
            if (msg.event_type === "sync_notify") {
                // Trigger HTMX refresh van notification panel
                htmx.trigger('#notification-list', 'refresh');
                this.updateBadge();
            }
        };
    }

    updateBadge() {
        fetch('/api/notifications/unread-count')
            .then(r => r.json())
            .then(data => {
                document.querySelector('#notif-badge').textContent = data.count;
            });
    }
}

// Start bij page load
document.addEventListener('DOMContentLoaded', () => {
    if (window.currentUserId) {
        new NotificationSocket();
    }
});
```

#### Stap 3: HTMX Refresh Endpoint

**Bestaand:** `/notifications` rendert de volledige pagina
**Nieuw:** `/partials/notification-list` rendert alleen de lijst

```rust
pub async fn notification_list_partial(...) -> Html<String> {
    // Zelfde query als notifications_handler
    // Return alleen de <ul> met notificaties
}
```

---

## 4. Implementatie Taken

### Must Have (MVP)

| # | Taak | Effort | File |
|---|------|--------|------|
| 1 | Ticket endpoint toevoegen | 2h | `web/routes/bus_ticket.rs` |
| 2 | JS WebSocket client | 3h | `templates/js/notifications.js` |
| 3 | HTMX partial endpoint | 1h | `web/routes/notifications.rs` |
| 4 | Badge update endpoint | 30m | `web/routes/notifications.rs` |
| 5 | Config: BUS_URL, SERVICE_TOKEN | 30m | `config.rs` |

### Nice to Have

| # | Taak | Effort |
|---|------|--------|
| 6 | Toast/popup bij nieuwe notificatie | 2h |
| 7 | Sound notification | 1h |
| 8 | Browser push fallback | 4h |
| 9 | Connection status indicator | 1h |

---

## 5. Configuratie Benodigdheden

### Environment Variables (Web App)

```env
# WebSocket Bus
WEBSOCKET_BUS_URL=http://websocket-bus:8000
SERVICE_TOKEN=<shared-secret>
```

### Kubernetes Config

De web app moet `SERVICE_TOKEN` hebben dat ook in de websocket-bus allowlist staat.

---

## 6. Test Plan

### Unit Tests

1. Ticket endpoint retourneert geldige ticket
2. WebSocket connect met ticket lukt
3. Subscribe op notifications topic lukt

### E2E Test

```
1. Abbas logt in op web
2. WebSocket verbindt
3. Emma stuurt friend request
4. notifications-service publiceert sync_notify
5. Abbas' browser ontvangt event
6. Notificatie badge update
7. Panel refresh via HTMX
```

---

## 7. Risico's & Mitigaties

| Risico | Impact | Mitigatie |
|--------|--------|-----------|
| WS connectie faalt | Geen real-time | Fallback naar polling (60s) |
| Ticket expired | Geen events | Auto-reconnect met nieuw ticket |
| Browser tab inactive | Gemiste events | Refresh bij tab focus |

---

## 8. Conclusie

De notifications-service is **volledig klaar** en publiceert al naar websocket-bus. De gap zit in de **consumer kant**:

1. **Web app** heeft geen ticket endpoint
2. **Frontend** heeft geen WebSocket client
3. **UI** heeft geen real-time update mechanisme

Geschatte totale effort: **~8 uur** voor MVP
