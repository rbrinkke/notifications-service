use std::env;

/// Debug configuratie - ultra logging voor development/troubleshooting
#[derive(Debug, Clone)]
pub struct DebugConfig {
    /// Master switch voor debug mode (DEBUG_MODE env var)
    pub enabled: bool,
    /// Log volledige notification payloads (DEBUG_LOG_PAYLOADS)
    pub log_payloads: bool,
    /// Log SQL queries met parameters (DEBUG_LOG_SQL)
    pub log_sql: bool,
    /// Log FCM tokens - SECURITY SENSITIVE! (DEBUG_LOG_FCM_TOKENS)
    pub log_fcm_tokens: bool,
    /// Log timing voor alle operaties (DEBUG_LOG_TIMING)
    pub log_timing: bool,
}

impl DebugConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: env::var("DEBUG_MODE")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            log_payloads: env::var("DEBUG_LOG_PAYLOADS")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            log_sql: env::var("DEBUG_LOG_SQL")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            log_fcm_tokens: env::var("DEBUG_LOG_FCM_TOKENS")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            log_timing: env::var("DEBUG_LOG_TIMING")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(true), // Default true - timing is always useful
        }
    }

    /// Check of debug logging actief is voor een bepaald niveau
    pub fn should_log_detail(&self) -> bool {
        self.enabled
    }
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            log_payloads: false,
            log_sql: false,
            log_fcm_tokens: false,
            log_timing: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    // Database
    pub database_url: String,

    // HTTP Server (health + metrics only, no WS)
    pub server_host: String,
    pub server_port: u16,

    // WebSocket Bus (unified real-time messaging)
    pub websocket_bus_url: Option<String>,
    pub service_token: Option<String>,

    // FCM Push
    pub fcm_project_id: Option<String>,
    pub fcm_credentials_path: Option<String>,

    // Worker
    pub worker_poll_interval_secs: u64,
    pub worker_batch_size: i64,
    pub max_retries: i32,

    // Debug
    pub debug: DebugConfig,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5441/activitydb".into()),

            server_host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            server_port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8080),

            // WebSocket Bus configuration
            websocket_bus_url: env::var("WEBSOCKET_BUS_URL").ok(),
            service_token: env::var("SERVICE_TOKEN").ok(),

            fcm_project_id: env::var("FCM_PROJECT_ID").ok(),
            fcm_credentials_path: env::var("GOOGLE_APPLICATION_CREDENTIALS").ok(),

            worker_poll_interval_secs: env::var("WORKER_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
            worker_batch_size: env::var("WORKER_BATCH_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),

            max_retries: env::var("MAX_RETRIES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),

            debug: DebugConfig::from_env(),
        }
    }

    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.server_host, self.server_port)
    }

    /// Check if websocket-bus is configured
    pub fn has_bus(&self) -> bool {
        self.websocket_bus_url.is_some() && self.service_token.is_some()
    }
}
