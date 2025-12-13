use crate::models::Notification;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// FCM HTTP v1 API Client
pub struct FcmClient {
    client: Client,
    project_id: String,
    service_account: ServiceAccount,
    /// Cached access token with expiry
    token_cache: Arc<RwLock<Option<CachedToken>>>,
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at: u64,
    obtained_at: u64,
}

#[derive(Debug, Deserialize)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
    project_id: String,
}

#[derive(Debug, Serialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: u64,
    exp: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Serialize)]
struct FcmRequest {
    message: FcmMessage,
}

#[derive(Debug, Serialize)]
struct FcmMessage {
    token: String,
    notification: FcmNotification,
    data: std::collections::HashMap<String, String>,
    android: AndroidConfig,
    apns: ApnsConfig,
}

#[derive(Debug, Serialize)]
struct FcmNotification {
    title: String,
    body: String,
}

#[derive(Debug, Serialize)]
struct AndroidConfig {
    priority: String,
}

#[derive(Debug, Serialize)]
struct ApnsConfig {
    payload: ApnsPayload,
}

#[derive(Debug, Serialize)]
struct ApnsPayload {
    aps: Aps,
}

#[derive(Debug, Serialize)]
struct Aps {
    sound: String,
    badge: i32,
    #[serde(rename = "content-available")]
    content_available: i32,
}

#[derive(Debug)]
pub enum FcmError {
    NotInitialized,
    TokenError(String),
    SendError(String),
    InvalidToken,
}

impl std::fmt::Display for FcmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FcmError::NotInitialized => write!(f, "FCM client not initialized"),
            FcmError::TokenError(e) => write!(f, "OAuth token error: {}", e),
            FcmError::SendError(e) => write!(f, "FCM send error: {}", e),
            FcmError::InvalidToken => write!(f, "Invalid FCM device token"),
        }
    }
}

impl FcmClient {
    /// Create new FCM client from service account file
    pub fn new(credentials_path: &str, project_id: &str) -> Result<Self, String> {
        debug!(
            credentials_path = %credentials_path,
            project_id = %project_id,
            "Initializing FCM client..."
        );

        trace!("Reading credentials file: {}", credentials_path);
        let content = std::fs::read_to_string(credentials_path)
            .map_err(|e| {
                error!(
                    path = %credentials_path,
                    error = %e,
                    "Failed to read FCM credentials file"
                );
                format!("Failed to read credentials: {}", e)
            })?;

        trace!("Parsing service account JSON...");
        let service_account: ServiceAccount = serde_json::from_str(&content)
            .map_err(|e| {
                error!(error = %e, "Failed to parse FCM credentials JSON");
                format!("Failed to parse credentials: {}", e)
            })?;

        info!(
            project_id = %project_id,
            client_email = %service_account.client_email,
            "✓ FCM client initialized"
        );

        Ok(Self {
            client: Client::new(),
            project_id: project_id.to_string(),
            service_account,
            token_cache: Arc::new(RwLock::new(None)),
        })
    }

    /// Get valid OAuth2 access token (cached or fresh)
    async fn get_access_token(&self) -> Result<String, FcmError> {
        trace!("Checking OAuth2 token cache...");

        // Check cache first
        {
            let cache = self.token_cache.read().await;
            if let Some(cached) = cache.as_ref() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                let time_remaining = cached.expires_at.saturating_sub(now);
                let age = now.saturating_sub(cached.obtained_at);

                // Use token if not expired (with 60s buffer)
                if cached.expires_at > now + 60 {
                    trace!(
                        age_secs = age,
                        remaining_secs = time_remaining,
                        "Using cached OAuth2 token"
                    );
                    return Ok(cached.access_token.clone());
                }

                debug!(
                    age_secs = age,
                    remaining_secs = time_remaining,
                    "Cached token expired or expiring soon, refreshing..."
                );
            } else {
                debug!("No cached token, fetching fresh OAuth2 token...");
            }
        }

        // Need fresh token
        let start = Instant::now();
        let token = self.fetch_access_token().await?;
        let duration = start.elapsed();

        debug!(
            duration_ms = duration.as_millis() as u64,
            expires_in_secs = token.expires_at - token.obtained_at,
            "Fresh OAuth2 token obtained"
        );

        // Cache it
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(token.clone());
            trace!("Token cached successfully");
        }

        Ok(token.access_token)
    }

    /// Fetch new OAuth2 token from Google
    async fn fetch_access_token(&self) -> Result<CachedToken, FcmError> {
        trace!("Building JWT for OAuth2 token exchange...");
        let start = Instant::now();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = JwtClaims {
            iss: self.service_account.client_email.clone(),
            scope: FCM_SCOPE.to_string(),
            aud: TOKEN_URL.to_string(),
            iat: now,
            exp: now + 3600, // 1 hour
        };

        trace!(
            iss = %claims.iss,
            scope = %claims.scope,
            iat = claims.iat,
            exp = claims.exp,
            "JWT claims prepared"
        );

        // Sign JWT with service account private key
        trace!("Signing JWT with RSA-256...");
        let key = EncodingKey::from_rsa_pem(self.service_account.private_key.as_bytes())
            .map_err(|e| {
                error!(error = %e, "Failed to parse RSA private key");
                FcmError::TokenError(format!("Invalid private key: {}", e))
            })?;

        let jwt = encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map_err(|e| {
                error!(error = %e, "JWT encoding failed");
                FcmError::TokenError(format!("JWT encoding failed: {}", e))
            })?;

        let jwt_time = start.elapsed();
        trace!(
            jwt_len = jwt.len(),
            duration_ms = jwt_time.as_millis() as u64,
            "JWT created and signed"
        );

        // Exchange JWT for access token
        trace!("Exchanging JWT for OAuth2 access token...");
        let exchange_start = Instant::now();

        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| {
                error!(
                    error = %e,
                    duration_ms = exchange_start.elapsed().as_millis() as u64,
                    "OAuth2 token request failed"
                );
                FcmError::TokenError(format!("Token request failed: {}", e))
            })?;

        let status = response.status();
        trace!(status = %status, "OAuth2 response received");

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(
                status = %status,
                body = %body,
                duration_ms = exchange_start.elapsed().as_millis() as u64,
                "OAuth2 token request failed"
            );
            return Err(FcmError::TokenError(format!(
                "Token request failed: {} - {}",
                status, body
            )));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to parse OAuth2 token response");
                FcmError::TokenError(format!("Token parse failed: {}", e))
            })?;

        let total_duration = start.elapsed();

        debug!(
            expires_in_secs = token_response.expires_in,
            duration_ms = total_duration.as_millis() as u64,
            "OAuth2 token exchange successful"
        );

        Ok(CachedToken {
            access_token: token_response.access_token,
            expires_at: now + token_response.expires_in,
            obtained_at: now,
        })
    }

    /// Send push notification to a single device
    pub async fn send(
        &self,
        fcm_token: &str,
        notification: &Notification,
    ) -> Result<(), FcmError> {
        let start = Instant::now();
        let token_preview = mask_token(fcm_token);

        trace!(
            token = %token_preview,
            notification_id = %notification.notification_id,
            notification_type = %notification.notification_type,
            "Sending FCM push notification..."
        );

        // Get OAuth2 token
        let token_start = Instant::now();
        let access_token = self.get_access_token().await?;
        let token_time = token_start.elapsed();
        trace!(
            duration_ms = token_time.as_millis() as u64,
            "OAuth2 token retrieved"
        );

        let url = format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            self.project_id
        );

        // Build request data
        let mut data = std::collections::HashMap::new();
        data.insert(
            "notification_id".to_string(),
            notification.notification_id.to_string(),
        );
        data.insert(
            "type".to_string(),
            notification.notification_type.clone(),
        );
        if let Some(deep_link) = &notification.deep_link {
            data.insert("deep_link".to_string(), deep_link.clone());
        }

        let priority = notification.priority.as_deref().unwrap_or("normal");
        let android_priority = if priority == "high" || priority == "critical" {
            "high"
        } else {
            "normal"
        };

        let request = FcmRequest {
            message: FcmMessage {
                token: fcm_token.to_string(),
                notification: FcmNotification {
                    title: notification.title.clone(),
                    body: notification.message.clone().unwrap_or_default(),
                },
                data,
                android: AndroidConfig {
                    priority: android_priority.to_string(),
                },
                apns: ApnsConfig {
                    payload: ApnsPayload {
                        aps: Aps {
                            sound: "default".to_string(),
                            badge: 1,
                            content_available: 1,
                        },
                    },
                },
            },
        };

        trace!(
            title = %notification.title,
            body = notification.message.as_deref().unwrap_or(""),
            android_priority = %android_priority,
            "FCM request payload prepared"
        );

        // Send request
        let send_start = Instant::now();
        let response = self
            .client
            .post(&url)
            .bearer_auth(&access_token)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                error!(
                    token = %token_preview,
                    error = %e,
                    duration_ms = send_start.elapsed().as_millis() as u64,
                    "FCM HTTP request failed"
                );
                FcmError::SendError(format!("Request failed: {}", e))
            })?;

        let status = response.status();
        let send_time = send_start.elapsed();
        let total_time = start.elapsed();

        trace!(
            status = %status,
            send_duration_ms = send_time.as_millis() as u64,
            "FCM response received"
        );

        if status.is_success() {
            debug!(
                token = %token_preview,
                status = %status,
                total_duration_ms = total_time.as_millis() as u64,
                send_duration_ms = send_time.as_millis() as u64,
                "✓ FCM push sent successfully"
            );
            return Ok(());
        }

        let body = response.text().await.unwrap_or_default();

        // Check for invalid token errors
        if body.contains("UNREGISTERED") || body.contains("INVALID_ARGUMENT") {
            warn!(
                token = %token_preview,
                status = %status,
                body = %body,
                duration_ms = total_time.as_millis() as u64,
                "FCM token is invalid (UNREGISTERED/INVALID_ARGUMENT)"
            );
            return Err(FcmError::InvalidToken);
        }

        error!(
            token = %token_preview,
            status = %status,
            body = %body,
            duration_ms = total_time.as_millis() as u64,
            "FCM send failed"
        );
        Err(FcmError::SendError(format!("{}: {}", status, body)))
    }
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
