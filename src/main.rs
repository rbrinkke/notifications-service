use axum::{routing::get, Router};
use bus_client::BusClient;
use notifications_service::config::Config;
use notifications_service::db::{Database, NotificationListener};
use notifications_service::push::FcmClient;
use notifications_service::worker::NotificationWorker;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Load configuration FIRST (before logging, to know debug mode)
    let config = Config::from_env();

    // Initialize logging based on debug mode
    init_logging(&config);

    info!("═══════════════════════════════════════════════════════════");
    info!("  NOTIFICATIONS SERVICE STARTING");
    info!("═══════════════════════════════════════════════════════════");

    // Log debug configuration
    if config.debug.enabled {
        warn!("DEBUG MODE ENABLED - verbose logging active");
        debug!("Debug config:");
        debug!("  log_payloads: {}", config.debug.log_payloads);
        debug!("  log_sql: {}", config.debug.log_sql);
        debug!("  log_fcm_tokens: {}", config.debug.log_fcm_tokens);
        debug!("  log_timing: {}", config.debug.log_timing);
    }
    info!(
        server_addr = %config.server_addr(),
        poll_interval = config.worker_poll_interval_secs,
        batch_size = config.worker_batch_size,
        max_retries = config.max_retries,
        "Configuration loaded"
    );
    trace!("Full config: {:?}", config);

    // Connect to database
    debug!("Connecting to database...");
    let start = std::time::Instant::now();
    let db = match Database::connect(&config.database_url).await {
        Ok(db) => {
            let duration = start.elapsed();
            info!(duration_ms = duration.as_millis() as u64, "Database connected");
            db
        }
        Err(e) => {
            error!(error = %e, "Failed to connect to database");
            std::process::exit(1);
        }
    };

    // Initialize FCM client (optional)
    debug!("Initializing FCM client...");
    let fcm_client = match (&config.fcm_credentials_path, &config.fcm_project_id) {
        (Some(path), Some(project_id)) => {
            trace!("FCM credentials path: {}", path);
            trace!("FCM project ID: {}", project_id);
            match FcmClient::new(path, project_id) {
                Ok(client) => {
                    info!(project_id = %project_id, "FCM client initialized");
                    Some(Arc::new(client))
                }
                Err(e) => {
                    error!(error = %e, path = %path, "Failed to initialize FCM client - push disabled");
                    None
                }
            }
        }
        _ => {
            warn!("FCM not configured - push notifications disabled");
            debug!("  FCM_PROJECT_ID: {:?}", config.fcm_project_id);
            debug!("  GOOGLE_APPLICATION_CREDENTIALS: {:?}", config.fcm_credentials_path);
            None
        }
    };

    // Initialize BusClient for websocket-bus
    debug!("Initializing WebSocket Bus client...");
    let bus_client = match (&config.websocket_bus_url, &config.service_token) {
        (Some(url), Some(token)) => {
            let client = BusClient::new(url, token);
            info!(bus_url = %url, "WebSocket Bus client initialized");
            Some(Arc::new(client))
        }
        _ => {
            warn!("WebSocket Bus not configured - real-time delivery disabled");
            debug!("  WEBSOCKET_BUS_URL: {:?}", config.websocket_bus_url);
            debug!("  SERVICE_TOKEN: {:?}", config.service_token.as_ref().map(|_| "[REDACTED]"));
            None
        }
    };

    // Channel for NOTIFY signals to worker
    debug!("Creating wake channel (buffer size: 10)...");
    let (wake_tx, wake_rx) = mpsc::channel::<()>(10);

    // Start Postgres NOTIFY listener
    debug!("Starting NOTIFY listener...");
    let listener = NotificationListener::new(config.database_url.clone());
    let listener_handle = tokio::spawn(async move {
        if let Err(e) = listener.listen(wake_tx).await {
            error!(error = %e, "NOTIFY listener failed");
        }
    });
    info!("NOTIFY listener started");

    // Start worker
    debug!("Starting notification worker...");
    let fcm_enabled = fcm_client.is_some();
    let worker = NotificationWorker::new(
        &db,
        config.clone(),
        bus_client.clone(),
        fcm_client,
    );
    let worker_handle = tokio::spawn(async move {
        worker.run(wake_rx).await;
    });
    info!(
        poll_interval_secs = config.worker_poll_interval_secs,
        batch_size = config.worker_batch_size,
        "Notification worker started"
    );

    // Start HTTP server (health + metrics only)
    debug!("Starting HTTP server...");
    let router = Router::new()
        .route("/health", get(health_handler))
        .route("/healthz", get(health_handler))
        .route("/readyz", get(health_handler))
        .route("/metrics", get(metrics_handler));

    let addr = config.server_addr();

    let tcp_listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            debug!("TCP listener bound to {}", addr);
            l
        }
        Err(e) => {
            error!(error = %e, addr = %addr, "Failed to bind HTTP server");
            std::process::exit(1);
        }
    };

    info!("═══════════════════════════════════════════════════════════");
    info!("  SERVICE READY");
    info!("  Health:    http://{}/health", addr);
    info!("  Metrics:   http://{}/metrics", addr);
    info!("  Bus:       {}", if bus_client.is_some() { "ENABLED" } else { "DISABLED" });
    info!("  FCM:       {}", if fcm_enabled { "ENABLED" } else { "DISABLED" });
    info!("═══════════════════════════════════════════════════════════");

    // Run server with graceful shutdown
    let server_handle = tokio::spawn(async move {
        axum::serve(tcp_listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .expect("Server failed");
    });

    // Wait for any task to complete (shouldn't happen normally)
    tokio::select! {
        _ = listener_handle => {
            error!("NOTIFY listener stopped unexpectedly");
        }
        _ = worker_handle => {
            error!("Worker stopped unexpectedly");
        }
        _ = server_handle => {
            info!("Server shutdown complete");
        }
    }

    info!("═══════════════════════════════════════════════════════════");
    info!("  NOTIFICATIONS SERVICE STOPPED");
    info!("═══════════════════════════════════════════════════════════");
}

async fn health_handler() -> &'static str {
    "OK"
}

async fn metrics_handler() -> String {
    // Basic Prometheus metrics
    let output = metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder();
    // For now, return empty metrics - can be expanded later
    "# notifications_service metrics\n".to_string()
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C, shutting down"),
        _ = terminate => info!("Received SIGTERM, shutting down"),
    }
}

/// Initialize logging based on debug configuration
fn init_logging(config: &Config) {
    use tracing_subscriber::fmt;

    // Determine log level based on DEBUG_MODE
    let env_filter = if config.debug.enabled {
        // In debug mode: use trace level for our crate, debug for others
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| {
                "notifications_service=trace,tower_http=debug,axum=debug,sqlx=debug,bus_client=debug".into()
            })
    } else {
        // Production: use RUST_LOG or default to info
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "notifications_service=info,bus_client=info".into())
    };

    if config.debug.enabled {
        // Debug mode: JSON structured logging for better parsing
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_list(true)
                    .with_file(true)
                    .with_line_number(true)
                    .with_thread_ids(true)
                    .with_target(true)
            )
            .init();
    } else {
        // Production: compact human-readable format
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                fmt::layer()
                    .compact()
                    .with_target(true)
                    .with_thread_ids(false)
            )
            .init();
    }
}
