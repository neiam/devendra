use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use clap::Parser;
use devendra::common::*;
use devendra::server::{self, ServerConfig};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Parser)]
#[command(name = "devendra-server")]
#[command(about = "Devendra configuration management server", long_about = None)]
struct Cli {
    /// Path to server configuration file
    #[arg(
        short,
        long,
        env = "CONFIG_PATH",
        default_value = "/etc/devendra/server.toml"
    )]
    config: PathBuf,

    /// Data directory for repos and database
    #[arg(short, long, env = "DATA_DIR", default_value = "/var/lib/devendra")]
    data_dir: PathBuf,
}

// Server state
#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    configurations: Arc<RwLock<HashMap<String, Configuration>>>,
    compositions: Arc<RwLock<HashMap<String, Composition>>>,
    personas: Arc<RwLock<HashMap<String, Persona>>>,
    registration_tokens: Arc<RwLock<HashMap<String, RegistrationToken>>>,
    // Maps configuration name to git revision
    config_revisions: Arc<RwLock<HashMap<String, String>>>,
}

#[tokio::main]
async fn main() {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    tracing::info!("Devendra Server starting...");
    tracing::info!("Data directory: {}", cli.data_dir.display());

    // Derive database path from data directory
    let db_path = cli.data_dir.join("devendra.db");
    let repo_base_path = cli.data_dir.join("repos");

    // Load or create server configuration
    let config_path = cli.config;
    let config: ServerConfig = if config_path.exists() {
        match load_toml(&config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    "Failed to load server configuration from {}: {}",
                    config_path.display(),
                    e
                );
                tracing::error!("Exiting...");
                std::process::exit(1);
            }
        }
    } else {
        tracing::info!("Configuration file not found at {}", config_path.display());
        tracing::info!("Generating default configuration...");

        let default_config = ServerConfig::default();

        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::error!("Failed to create config directory: {}", e);
                tracing::error!("Exiting...");
                std::process::exit(1);
            }
        }

        // Save default configuration
        match save_toml(&config_path, &default_config) {
            Ok(_) => {
                tracing::info!("Default configuration saved to {}", config_path.display());
                tracing::info!(
                    "Please edit this file to add your git repositories and adjust settings"
                );
            }
            Err(e) => {
                tracing::error!("Failed to save default configuration: {}", e);
                tracing::error!("Exiting...");
                std::process::exit(1);
            }
        }

        default_config
    };

    tracing::info!("Server ID: {}", config.id);
    tracing::info!("Bind address: {}", config.bind_address);
    tracing::info!("Database path: {}", db_path.display());
    tracing::info!("Git repositories: {}", config.git_repos.len());
    tracing::info!("Sync interval: {} seconds", config.sync_interval_secs);

    // Initialize database
    tracing::info!("Initializing database...");
    let db = init_database(&db_path.to_string_lossy())
        .await
        .expect("Failed to initialize database");

    // Initialize shared state
    let state = AppState {
        db,
        configurations: Arc::new(RwLock::new(HashMap::new())),
        compositions: Arc::new(RwLock::new(HashMap::new())),
        personas: Arc::new(RwLock::new(HashMap::new())),
        registration_tokens: Arc::new(RwLock::new(HashMap::new())),
        config_revisions: Arc::new(RwLock::new(HashMap::new())),
    };

    // Clone for git sync task
    let sync_config = config.clone();
    let sync_state = state.clone();
    let sync_repo_path = repo_base_path.clone();

    // Start git sync task
    tokio::spawn(async move {
        loop {
            tracing::debug!("Git Sync Cycle");

            for (idx, repo) in sync_config.git_repos.iter().enumerate() {
                let repo_path = sync_repo_path.join(format!("repo_{}", idx));

                tracing::info!("Syncing repository: {}", repo.url);
                match server::sync_git_repository(repo, &repo_path) {
                    Ok(revision) => {
                        tracing::info!("Repository synced successfully. Revision: {}", revision);

                        // Load configurations and track their revision
                        if let Ok(configs) = server::load_configurations_from_repo(&repo_path) {
                            tracing::info!("Loaded {} configurations", configs.len());
                            let mut cfg = sync_state.configurations.write().await;
                            let mut revisions = sync_state.config_revisions.write().await;
                            for (name, config) in configs {
                                revisions.insert(name.clone(), revision.clone());
                                cfg.insert(name, config);
                            }
                        }

                        // Load compositions
                        if let Ok(comps) = server::load_compositions_from_repo(&repo_path) {
                            tracing::info!("Loaded {} compositions", comps.len());
                            let mut cmp = sync_state.compositions.write().await;
                            cmp.extend(comps);
                        }

                        // Load personas
                        if let Ok(pers) = server::load_personas_from_repo(&repo_path) {
                            tracing::info!("Loaded {} personas", pers.len());
                            let mut per = sync_state.personas.write().await;
                            per.extend(pers);
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to sync repository {}: {:?}", repo.url, e);
                    }
                }
            }

            tracing::debug!(
                "Git sync complete. Sleeping for {} seconds...",
                sync_config.sync_interval_secs
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(
                sync_config.sync_interval_secs,
            ))
            .await;
        }
    });

    // Build API router
    let app = Router::new()
        .route("/api/register", post(handle_register))
        .route("/api/check", post(handle_check))
        .route("/api/telemetry", post(handle_telemetry))
        .route("/api/result", post(handle_result))
        .with_state(state);

    tracing::info!("Starting API Server");
    tracing::info!("Listening on {}", config.bind_address);

    let listener = tokio::net::TcpListener::bind(&config.bind_address)
        .await
        .expect("Failed to bind to address");

    axum::serve(listener, app).await.expect("Server failed");
}

// Database initialization
async fn init_database(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    // Create parent directory if it doesn't exist
    if let Some(parent) = std::path::Path::new(db_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);

    let pool = SqlitePool::connect_with(options).await?;

    // Create tables
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            public_key TEXT NOT NULL,
            hostname TEXT NOT NULL,
            auth_token TEXT NOT NULL UNIQUE,
            persona_name TEXT NOT NULL,
            registered_at INTEGER NOT NULL,
            last_seen INTEGER
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS telemetry (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            hostname TEXT NOT NULL,
            total_bytes INTEGER NOT NULL,
            used_bytes INTEGER NOT NULL,
            available_bytes INTEGER NOT NULL,
            uptime_secs INTEGER NOT NULL,
            timestamp INTEGER NOT NULL,
            FOREIGN KEY (agent_id) REFERENCES agents(id)
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS configuration_results (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            configuration_name TEXT NOT NULL,
            status TEXT NOT NULL,
            error_message TEXT,
            timestamp INTEGER NOT NULL,
            retry_count INTEGER NOT NULL,
            FOREIGN KEY (agent_id) REFERENCES agents(id)
        )
        "#,
    )
    .execute(&pool)
    .await?;

    tracing::info!("Database initialized successfully");
    Ok(pool)
}

// Handler: Agent registration
async fn handle_register(
    State(state): State<AppState>,
    Json(req): Json<RegistrationRequest>,
) -> Result<Json<RegistrationResponse>, StatusCode> {
    tracing::info!("Registration request from hostname: {}", req.hostname);

    // Verify registration token exists
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let is_single_use = {
        let tokens = state.registration_tokens.read().await;
        let token_data = tokens.get(&req.token).ok_or(StatusCode::UNAUTHORIZED)?;

        // Check if token is expired
        if token_data.expires_at < now {
            return Err(StatusCode::UNAUTHORIZED);
        }

        token_data.single_use
    };

    // Generate agent ID and auth token
    let agent_id = uuid7::uuid7();
    let auth_token = uuid7::uuid7().to_string();

    // Store agent in database
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    sqlx::query(
        r#"
        INSERT INTO agents (id, public_key, hostname, auth_token, persona_name, registered_at)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(agent_id.to_string())
    .bind(&req.public_key)
    .bind(&req.hostname)
    .bind(&auth_token)
    .bind(&req.persona_name)
    .bind(now)
    .execute(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Remove single-use token
    if is_single_use {
        let mut tokens = state.registration_tokens.write().await;
        tokens.remove(&req.token);
    }

    tracing::info!("Agent {} registered successfully", agent_id);

    Ok(Json(RegistrationResponse {
        auth_token,
        agent_id,
    }))
}

// Handler: Configuration check
async fn handle_check(
    State(state): State<AppState>,
    Json(req): Json<ConfigurationCheckRequest>,
) -> Result<Json<ConfigurationCheckResponse>, StatusCode> {
    tracing::debug!("Configuration check from agent: {}", req.agent_id);

    // Verify agent exists and get persona
    let agent_row: Option<(String,)> =
        sqlx::query_as("SELECT persona_name FROM agents WHERE id = ?")
            .bind(req.agent_id.to_string())
            .fetch_optional(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let persona_name = match agent_row {
        Some((name,)) => name,
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    // Update last_seen timestamp
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    sqlx::query("UPDATE agents SET last_seen = ? WHERE id = ?")
        .bind(now)
        .bind(req.agent_id.to_string())
        .execute(&state.db)
        .await
        .ok();

    // Get persona configurations
    let personas = state.personas.read().await;
    let persona = match personas.get(&persona_name) {
        Some(p) => p,
        None => {
            tracing::warn!("Persona '{}' not found", persona_name);
            return Ok(Json(ConfigurationCheckResponse {
                updates_available: vec![],
            }));
        }
    };

    // Get all compositions for this persona
    let compositions = state.compositions.read().await;
    let configurations = state.configurations.read().await;
    let revisions = state.config_revisions.read().await;

    let mut all_config_names = Vec::new();
    for composition_name in &persona.compositions {
        if let Some(composition) = compositions.get(composition_name) {
            all_config_names.extend(composition.configurations.clone());
        }
    }

    // Build configuration updates - only send if revision changed
    let mut updates = Vec::new();
    for config_name in all_config_names {
        if let Some(config) = configurations.get(&config_name) {
            if let Some(new_revision) = revisions.get(&config_name) {
                // Check if agent has this revision already
                let needs_update = match req.current_revisions.get(&config_name) {
                    Some(agent_revision) => agent_revision != new_revision,
                    None => true, // Agent doesn't have this config at all
                };

                if needs_update {
                    updates.push(ConfigurationUpdate {
                        configuration_name: config_name.clone(),
                        new_revision: new_revision.clone(),
                        configuration: config.clone(),
                    });
                }
            }
        }
    }

    tracing::info!(
        "Returning {} configuration updates for agent {}",
        updates.len(),
        req.agent_id
    );

    Ok(Json(ConfigurationCheckResponse {
        updates_available: updates,
    }))
}

// Handler: Telemetry
async fn handle_telemetry(
    State(state): State<AppState>,
    Json(telemetry): Json<AgentTelemetry>,
) -> StatusCode {
    tracing::debug!(
        "Telemetry from agent {}: hostname={}, uptime={}s",
        telemetry.agent_id,
        telemetry.hostname,
        telemetry.uptime_secs
    );

    // Store telemetry in database
    let result = sqlx::query(
        r#"
        INSERT INTO telemetry (agent_id, hostname, total_bytes, used_bytes, available_bytes, uptime_secs, timestamp)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(telemetry.agent_id.to_string())
    .bind(&telemetry.hostname)
    .bind(telemetry.disk_usage.total_bytes as i64)
    .bind(telemetry.disk_usage.used_bytes as i64)
    .bind(telemetry.disk_usage.available_bytes as i64)
    .bind(telemetry.uptime_secs as i64)
    .bind(telemetry.timestamp)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::error!("Failed to store telemetry: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

// Handler: Configuration application result
async fn handle_result(
    State(state): State<AppState>,
    Json(result): Json<ConfigurationApplicationResult>,
) -> StatusCode {
    tracing::info!(
        "Configuration result from agent {}: {} - {:?}",
        result.agent_id,
        result.configuration_name,
        result.status
    );

    // Convert status to string
    let status_str = match result.status {
        ApplicationStatus::Success => "success",
        ApplicationStatus::Failed => "failed",
        ApplicationStatus::Retrying { .. } => "retrying",
    };

    // Store result in database
    let db_result = sqlx::query(
        r#"
        INSERT INTO configuration_results (agent_id, configuration_name, status, error_message, timestamp, retry_count)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(result.agent_id.to_string())
    .bind(&result.configuration_name)
    .bind(status_str)
    .bind(result.error_message.as_deref())
    .bind(result.timestamp)
    .bind(result.retry_count as i64)
    .execute(&state.db)
    .await;

    match db_result {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::error!("Failed to store configuration result: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
