use devendra::agent::AgentConfig;
use devendra::common::*;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::collections::HashMap;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use base64::{Engine as _, engine::general_purpose};

#[derive(Parser)]
#[command(name = "devendra-agent")]
#[command(about = "Devendra configuration management agent", long_about = None)]
struct Cli {
    /// Path to agent configuration file
    #[arg(short, long, env = "CONFIG_PATH", default_value = "/etc/devendra/agent.toml")]
    config: PathBuf,

    /// Data directory for storing configurations
    #[arg(short, long, env = "DATA_DIR", default_value = "/var/lib/devendra")]
    data_dir: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Register the agent with the server
    Register {
        /// Registration token provided by the server
        #[arg(short, long)]
        token: String,

        /// Server URL to register with
        #[arg(short, long)]
        server_url: String,

        /// Persona name for this agent
        #[arg(short, long)]
        persona: String,
    },
    /// Run the agent main loop
    Run,
}

fn main() {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Register { token, server_url, persona } => {
            register_agent(&cli.config, token, server_url, persona);
        }
        Commands::Run => {
            run_agent(&cli.config, &cli.data_dir);
        }
    }
}

#[tokio::main]
async fn register_agent(config_path: &PathBuf, token: &str, server_url: &str, persona: &str) {
    tracing::info!("Devendra Agent - Registration");
    tracing::info!("Server URL: {}", server_url);
    tracing::info!("Persona: {}", persona);
    tracing::debug!("Registration token: {}", token);

    // Generate agent ID
    let agent_id = uuid7::uuid7();
    tracing::info!("Generated Agent ID: {}", agent_id);

    // Generate Ed25519 keypair
    tracing::info!("Generating Ed25519 keypair...");
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key: VerifyingKey = (&signing_key).into();

    // Encode keys as base64
    let private_key = general_purpose::STANDARD.encode(signing_key.to_bytes());
    let public_key = general_purpose::STANDARD.encode(verifying_key.to_bytes());

    tracing::info!("Public key: {}", public_key);

    // Send registration request to server
    tracing::info!("Sending registration request to {}...", server_url);
    let registration_request = RegistrationRequest {
        token: token.to_string(),
        public_key: public_key.clone(),
        hostname: hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string()),
        persona_name: persona.to_string(),
    };

    let client = reqwest::Client::new();
    let register_url = format!("{}/api/register", server_url);

    tracing::info!("Contacting server at {}...", register_url);
    let response = match client
        .post(&register_url)
        .json(&registration_request)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Failed to connect to server: {}", e);
            tracing::error!("Please check that the server URL is correct and the server is running.");
            std::process::exit(1);
        }
    };

    if !response.status().is_success() {
        tracing::error!("Registration failed with status: {}", response.status());
        if let Ok(body) = response.text().await {
            tracing::error!("Error: {}", body);
        }
        std::process::exit(1);
    }

    let registration_response: RegistrationResponse = match response.json().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to parse server response: {}", e);
            std::process::exit(1);
        }
    };

    let auth_token = registration_response.auth_token;

    // Create agent configuration
    let config = AgentConfig {
        id: agent_id,
        server_url: server_url.to_string(),
        private_key,
        auth_token: Some(auth_token),
        poll_interval_secs: 300, // 5 minutes
        max_retry_attempts: 3,
        persona_name: persona.to_string(),
    };

    // Save configuration
    match save_toml(config_path, &config) {
        Ok(_) => {
            tracing::info!("Registration successful!");
            tracing::info!("Configuration saved to: {}", config_path.display());
            tracing::info!("You can now run the agent with: devendra-agent run");
        }
        Err(e) => {
            tracing::error!("Failed to save configuration: {}", e);
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn run_agent(config_path: &PathBuf, data_dir: &PathBuf) {
    tracing::info!("Devendra Agent starting...");
    tracing::info!("Data directory: {}", data_dir.display());

    // Load agent configuration
    let config: AgentConfig = match load_toml(config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load agent configuration from {}: {}", config_path.display(), e);
            tracing::error!("Run 'devendra-agent register' first to register this agent.");
            std::process::exit(1);
        }
    };

    tracing::info!("Agent ID: {}", config.id);
    tracing::info!("Server URL: {}", config.server_url);
    tracing::info!("Persona: {}", config.persona_name);
    tracing::info!("Poll interval: {} seconds", config.poll_interval_secs);

    // Check if we have auth token
    if config.auth_token.is_none() {
        tracing::error!("No auth token found in configuration.");
        tracing::error!("The agent may not have been properly registered.");
        tracing::error!("Run 'devendra-agent register' to register this agent.");
        std::process::exit(1);
    }

    tracing::info!("Starting agent main loop...");

    let client = reqwest::Client::new();
    let mut current_revisions = std::collections::HashMap::new();

    // Main agent loop
    loop {
        tracing::debug!("Agent Poll Cycle");

        // Poll server for configuration updates
        if let Err(e) = poll_configurations(&client, &config, &mut current_revisions, data_dir).await {
            tracing::error!("Failed to poll configurations: {}", e);
        }

        // Collect and send telemetry
        if let Err(e) = send_telemetry(&client, &config).await {
            tracing::error!("Failed to send telemetry: {}", e);
        }

        tracing::debug!("Poll cycle complete. Sleeping for {} seconds...", config.poll_interval_secs);
        tokio::time::sleep(tokio::time::Duration::from_secs(config.poll_interval_secs)).await;
    }
}

async fn poll_configurations(
    client: &reqwest::Client,
    config: &AgentConfig,
    current_revisions: &mut HashMap<String, String>,
    data_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let check_url = format!("{}/api/check", config.server_url);

    let check_request = ConfigurationCheckRequest {
        agent_id: config.id,
        current_revisions: current_revisions.clone(),
    };

    let response = client
        .post(&check_url)
        .json(&check_request)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("Server returned status: {}", response.status()).into());
    }

    let check_response: ConfigurationCheckResponse = response.json().await?;

    if check_response.updates_available.is_empty() {
        tracing::debug!("No configuration updates available");
        return Ok(());
    }

    tracing::info!("Received {} configuration updates", check_response.updates_available.len());

    // Apply each configuration
    for update in check_response.updates_available {
        tracing::info!("Applying configuration: {}", update.configuration_name);

        // Create config directory for storing configuration files
        let config_dir = data_dir.join("configs").join(&update.configuration_name);
        std::fs::create_dir_all(&config_dir)?;

        // Execute configuration
        let result = match devendra::agent::execute_configuration(&update.configuration, &config_dir) {
            Ok(needs_reboot) => {
                tracing::info!("Configuration applied successfully. Needs reboot: {}", needs_reboot);
                current_revisions.insert(update.configuration_name.clone(), update.new_revision.clone());

                ConfigurationApplicationResult {
                    agent_id: config.id,
                    configuration_name: update.configuration_name.clone(),
                    status: ApplicationStatus::Success,
                    error_message: None,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64,
                    retry_count: 0,
                }
            }
            Err(e) => {
                tracing::error!("Configuration failed: {}", e);
                ConfigurationApplicationResult {
                    agent_id: config.id,
                    configuration_name: update.configuration_name.clone(),
                    status: ApplicationStatus::Failed,
                    error_message: Some(e.to_string()),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64,
                    retry_count: 0,
                }
            }
        };

        // Send result to server
        let result_url = format!("{}/api/result", config.server_url);
        if let Err(e) = client.post(&result_url).json(&result).send().await {
            tracing::error!("Failed to send configuration result: {}", e);
        }
    }

    Ok(())
}

async fn send_telemetry(
    client: &reqwest::Client,
    config: &AgentConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get hostname
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    // Get disk usage (using statvfs on /)
    let disk_usage = get_disk_usage()?;

    // Get uptime
    let uptime_secs = get_uptime()?;

    let telemetry = AgentTelemetry {
        agent_id: config.id,
        hostname,
        disk_usage,
        uptime_secs,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
    };

    let telemetry_url = format!("{}/api/telemetry", config.server_url);
    client.post(&telemetry_url).json(&telemetry).send().await?;

    tracing::debug!("Telemetry sent successfully");
    Ok(())
}

fn get_disk_usage() -> Result<DiskUsage, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::mem;

        // Use libc statvfs to get actual disk usage
        let path = CString::new("/")?;
        let mut stat: libc::statvfs = unsafe { mem::zeroed() };

        let result = unsafe { libc::statvfs(path.as_ptr(), &mut stat) };

        if result == 0 {
            let block_size = stat.f_frsize as u64;
            let total_bytes = stat.f_blocks * block_size;
            let available_bytes = stat.f_bavail * block_size;
            let used_bytes = total_bytes - (stat.f_bfree * block_size);

            Ok(DiskUsage {
                total_bytes,
                used_bytes,
                available_bytes,
            })
        } else {
            Err("Failed to get filesystem statistics".into())
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Fallback for non-Linux systems
        Ok(DiskUsage {
            total_bytes: 1000000000000,
            used_bytes: 500000000000,
            available_bytes: 500000000000,
        })
    }
}

fn get_uptime() -> Result<u64, Box<dyn std::error::Error>> {
    // Read from /proc/uptime on Linux
    let uptime_str = std::fs::read_to_string("/proc/uptime")?;
    let uptime_secs: f64 = uptime_str
        .split_whitespace()
        .next()
        .ok_or("Invalid uptime format")?
        .parse()?;

    Ok(uptime_secs as u64)
}
