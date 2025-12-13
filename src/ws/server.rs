use crate::ws::connection::handle_connection;
use crate::ws::manager::ConnectionManager;
use axum::{
    extract::{Query, State, WebSocketUpgrade},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// One-time WebSocket authentication ticket
#[derive(Debug, Clone)]
struct WsTicket {
    user_id: Uuid,
    created_at: Instant,
}

impl WsTicket {
    fn new(user_id: Uuid) -> Self {
        Self {
            user_id,
            created_at: Instant::now(),
        }
    }

    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > Duration::from_secs(30)
    }
}

/// Thread-safe ticket store
#[derive(Clone, Default)]
pub struct TicketStore {
    tickets: Arc<RwLock<HashMap<String, WsTicket>>>,
}

impl TicketStore {
    pub fn new() -> Self {
        Self {
            tickets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn create_ticket(&self, user_id: Uuid) -> String {
        let ticket_id = Uuid::new_v4().to_string();
        let ticket = WsTicket::new(user_id);

        let mut tickets = self.tickets.write().await;

        // Clean up expired tickets while we have the lock
        tickets.retain(|_, t| !t.is_expired());

        tickets.insert(ticket_id.clone(), ticket);
        debug!(ticket_id = %ticket_id, user_id = %user_id, "Created WS ticket");

        ticket_id
    }

    async fn validate_and_consume(&self, ticket_id: &str) -> Option<Uuid> {
        let mut tickets = self.tickets.write().await;

        if let Some(ticket) = tickets.remove(ticket_id) {
            if ticket.is_expired() {
                warn!(ticket_id = %ticket_id, "WS ticket expired");
                return None;
            }
            info!(ticket_id = %ticket_id, user_id = %ticket.user_id, "WS ticket validated and consumed");
            Some(ticket.user_id)
        } else {
            warn!(ticket_id = %ticket_id, "WS ticket not found or already used");
            None
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub connection_manager: ConnectionManager,
    pub ticket_store: TicketStore,
}

#[derive(Debug, Deserialize)]
pub struct WsParams {
    pub user_id: Option<Uuid>,
    pub token: Option<String>,
    pub ticket: Option<String>,
    pub last_event_id: Option<String>,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub connections: usize,
}

#[derive(Serialize)]
pub struct WsTicketResponse {
    pub ticket: String,
    pub expires_in: u32,
    pub ws_url: String,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

pub fn create_router(manager: ConnectionManager) -> Router {
    let state = Arc::new(AppState {
        connection_manager: manager,
        ticket_store: TicketStore::new(),
    });

    Router::new()
        // Legacy route (backwards compat)
        .route("/ws", get(ws_handler))
        // Flutter app expected routes
        .route("/api/v1/notifications/ws", get(ws_handler))
        .route("/api/v1/notifications/ws-ticket", post(ws_ticket_handler))
        // Health & metrics
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

/// Extract user_id from JWT Authorization header
/// For now, we trust the X-User-Id header set by the ingress/gateway
/// In production, this should validate the JWT properly
fn extract_user_id_from_headers(headers: &HeaderMap) -> Option<Uuid> {
    // First try X-User-Id header (set by API gateway after JWT validation)
    if let Some(user_id_header) = headers.get("x-user-id") {
        if let Ok(user_id_str) = user_id_header.to_str() {
            if let Ok(user_id) = Uuid::parse_str(user_id_str) {
                debug!(user_id = %user_id, "Got user_id from X-User-Id header");
                return Some(user_id);
            }
        }
    }

    // Try to extract from Authorization Bearer token
    if let Some(auth_header) = headers.get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                // Decode JWT without verification to get user_id
                // The token should already be validated by the API gateway
                if let Some(user_id) = decode_jwt_user_id(token) {
                    debug!(user_id = %user_id, "Got user_id from JWT");
                    return Some(user_id);
                }
            }
        }
    }

    None
}

/// Decode JWT to extract user_id (sub claim) without cryptographic verification
/// The JWT should already be validated by the API gateway
fn decode_jwt_user_id(token: &str) -> Option<Uuid> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    // Decode payload (second part)
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;

    // Extract user_id from "sub" claim
    let sub = payload.get("sub")?.as_str()?;
    Uuid::parse_str(sub).ok()
}

/// POST /api/v1/notifications/ws-ticket
/// Creates a one-time ticket for WebSocket authentication
async fn ws_ticket_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    info!("WS ticket request received");
    debug!("Headers: {:?}", headers.keys().collect::<Vec<_>>());

    let Some(user_id) = extract_user_id_from_headers(&headers) else {
        warn!("WS ticket request without valid authentication");
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Authentication required".to_string(),
                code: "UNAUTHORIZED".to_string(),
            }),
        )
            .into_response();
    };

    let ticket = state.ticket_store.create_ticket(user_id).await;

    info!(user_id = %user_id, ticket_preview = %&ticket[..8], "WS ticket created");

    Json(WsTicketResponse {
        ticket,
        expires_in: 30,
        ws_url: "/api/v1/notifications/ws".to_string(),
    })
    .into_response()
}

/// GET /ws or /api/v1/notifications/ws
/// WebSocket upgrade handler supporting both ticket-based and direct user_id auth
async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    debug!("WebSocket upgrade request: {:?}", params);

    // Try ticket-based authentication first
    if let Some(ticket) = &params.ticket {
        if let Some(user_id) = state.ticket_store.validate_and_consume(ticket).await {
            info!(user_id = %user_id, "WebSocket upgrade via ticket");
            let manager = state.connection_manager.clone();
            return ws
                .on_upgrade(move |socket| handle_connection(socket, user_id, manager))
                .into_response();
        } else {
            warn!("WebSocket upgrade with invalid/expired ticket");
            return (StatusCode::UNAUTHORIZED, "Invalid or expired ticket").into_response();
        }
    }

    // Fall back to direct user_id (for testing/legacy)
    if let Some(user_id) = params.user_id {
        info!(user_id = %user_id, "WebSocket upgrade via direct user_id");
        let manager = state.connection_manager.clone();
        return ws
            .on_upgrade(move |socket| handle_connection(socket, user_id, manager))
            .into_response();
    }

    warn!("WebSocket connection attempt without ticket or user_id");
    (StatusCode::BAD_REQUEST, "ticket or user_id required").into_response()
}

async fn health_handler(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let connections = state.connection_manager.total_connections().await;
    Json(HealthResponse {
        status: "ok",
        connections,
    })
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> String {
    let connections = state.connection_manager.total_connections().await;
    let users = state.connection_manager.connected_users().await.len();

    format!(
        "# HELP websocket_connections_active Active WebSocket connections\n\
         # TYPE websocket_connections_active gauge\n\
         websocket_connections_active {}\n\
         # HELP websocket_users_connected Connected users\n\
         # TYPE websocket_users_connected gauge\n\
         websocket_users_connected {}\n",
        connections, users
    )
}
