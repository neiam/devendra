use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use uuid7::Uuid;

// Core Data Structures

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RebootStrategy {
    No,
    Always,
    IfRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub enum CommandStage {
    PreDeps,
    PostDeps,
    PreFiles,
    PostFiles,
    After,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceAction {
    Stop,
    Start,
    Restart,
    Enable,
    Disable,
    EnableNow,
    DisableNow,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Configuration {
    pub desc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable_map: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_map: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_map: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<HashMap<CommandStage, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<HashMap<String, ServiceAction>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_map: Option<HashMap<String, Vec<String>>>,
    pub reboot: RebootStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Composition {
    pub desc: String,
    pub configurations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    pub id: Uuid,
    pub compositions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRepository {
    pub url: String,
    pub branch: String,
    pub ssh_key_path: Option<String>,
}

// Network Protocol Structures

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationToken {
    pub token: String,
    pub expires_at: i64,
    pub single_use: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub token: String,
    pub public_key: String,
    pub hostname: String,
    pub persona_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationResponse {
    pub auth_token: String,
    pub agent_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedRequest {
    pub agent_id: Uuid,
    pub auth_token: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTelemetry {
    pub agent_id: Uuid,
    pub hostname: String,
    pub disk_usage: DiskUsage,
    pub uptime_secs: u64,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskUsage {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApplicationStatus {
    Success,
    Failed,
    Retrying { attempt: u32, max_attempts: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationApplicationResult {
    pub agent_id: Uuid,
    pub configuration_name: String,
    pub status: ApplicationStatus,
    pub error_message: Option<String>,
    pub timestamp: i64,
    pub retry_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationCheckRequest {
    pub agent_id: Uuid,
    pub current_revisions: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationCheckResponse {
    pub updates_available: Vec<ConfigurationUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationUpdate {
    pub configuration_name: String,
    pub new_revision: String,
    pub configuration: Configuration,
}

// Utility Functions

/// Interpolates environment variables in the format ${VAR_NAME}
pub fn interpolate_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    let pattern = regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();

    for cap in pattern.captures_iter(input) {
        if let Some(var_name) = cap.get(1) {
            if let Ok(value) = env::var(var_name.as_str()) {
                result = result.replace(&cap[0], &value);
            }
        }
    }

    result
}

/// Load a TOML configuration from a file
pub fn load_toml<T: serde::de::DeserializeOwned>(path: &PathBuf) -> io::Result<T> {
    let contents = fs::read_to_string(path)?;
    toml::from_str(&contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Save a TOML configuration to a file
pub fn save_toml<T: serde::Serialize>(path: &PathBuf, data: &T) -> io::Result<()> {
    let contents =
        toml::to_string_pretty(data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, contents)
}

/// Load a Configuration from a TOML file
pub fn load_configuration(path: &PathBuf) -> io::Result<Configuration> {
    load_toml(path)
}

/// Load a Composition from a TOML file
pub fn load_composition(path: &PathBuf) -> io::Result<Composition> {
    load_toml(path)
}

/// Load a Persona from a TOML file
pub fn load_persona(path: &PathBuf) -> io::Result<Persona> {
    load_toml(path)
}
