use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use uuid7::Uuid;

use crate::common::*;

// Agent Configuration

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: Uuid,
    pub server_url: String,
    pub private_key: String,
    pub auth_token: Option<String>,
    pub poll_interval_secs: u64,
    pub max_retry_attempts: u32,
    pub persona_name: String,
}

// Execution Errors

#[derive(Debug)]
pub enum ExecutionError {
    Io(io::Error),
    CommandFailed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },
    DependencyInstallFailed {
        package: String,
        error: String,
    },
    ServiceActionFailed {
        service: String,
        action: String,
        error: String,
    },
    TemplateError(String),
    UnsupportedPackageManager(String),
}

impl From<io::Error> for ExecutionError {
    fn from(e: io::Error) -> Self {
        ExecutionError::Io(e)
    }
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionError::Io(e) => write!(f, "IO error: {}", e),
            ExecutionError::CommandFailed {
                command,
                exit_code,
                stderr,
            } => {
                write!(
                    f,
                    "Command '{}' failed with exit code {:?}: {}",
                    command, exit_code, stderr
                )
            }
            ExecutionError::DependencyInstallFailed { package, error } => {
                write!(f, "Failed to install package '{}': {}", package, error)
            }
            ExecutionError::ServiceActionFailed {
                service,
                action,
                error,
            } => {
                write!(f, "Failed to {} service '{}': {}", action, service, error)
            }
            ExecutionError::TemplateError(e) => write!(f, "Template error: {}", e),
            ExecutionError::UnsupportedPackageManager(pm) => {
                write!(f, "Unsupported package manager: {}", pm)
            }
        }
    }
}

impl std::error::Error for ExecutionError {}

// Configuration Execution

/// Detect the package manager on the system
fn detect_package_manager() -> Result<String, ExecutionError> {
    let managers = vec!["apt", "dnf", "yum", "zypper", "pacman", "brew"];

    for manager in managers {
        let output = Command::new("which").arg(manager).output();

        if let Ok(result) = output {
            if result.status.success() {
                return Ok(manager.to_string());
            }
        }
    }

    Err(ExecutionError::UnsupportedPackageManager(
        "No supported package manager found".to_string(),
    ))
}

/// Install dependencies using the system package manager
pub fn install_dependencies(
    dependency_map: &HashMap<String, Vec<String>>,
) -> Result<(), ExecutionError> {
    let package_manager = detect_package_manager()?;

    if let Some(packages) = dependency_map.get(&package_manager) {
        tracing::info!(
            "Installing dependencies using {}: {:?}",
            package_manager,
            packages
        );

        let install_cmd = match package_manager.as_str() {
            "apt" => vec!["apt-get", "install", "-y"],
            "dnf" | "yum" => vec![&package_manager, "install", "-y"],
            "zypper" => vec!["zypper", "install", "-y"],
            "pacman" => vec!["pacman", "-S", "--noconfirm"],
            "brew" => vec!["brew", "install"],
            _ => return Err(ExecutionError::UnsupportedPackageManager(package_manager)),
        };

        let mut cmd = Command::new("sudo");
        for arg in install_cmd {
            cmd.arg(arg);
        }
        for package in packages {
            cmd.arg(package);
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(ExecutionError::DependencyInstallFailed {
                package: packages.join(", "),
                error: stderr,
            });
        }

        tracing::info!("Successfully installed dependencies");
    }

    Ok(())
}

/// Copy files from the configuration directory to their destinations
pub fn copy_files(
    config_dir: &PathBuf,
    file_map: &HashMap<String, String>,
    _variable_map: &Option<HashMap<String, String>>,
) -> Result<(), ExecutionError> {
    for (source, dest) in file_map {
        let source_path = config_dir.join(source);
        let dest_path = PathBuf::from(interpolate_env_vars(dest));

        tracing::info!(
            "Copying {} to {}",
            source_path.display(),
            dest_path.display()
        );

        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::copy(&source_path, &dest_path)?;
    }

    Ok(())
}

/// Render templates and write them to their destinations
pub fn render_templates(
    config_dir: &PathBuf,
    template_map: &HashMap<String, String>,
    variable_map: &Option<HashMap<String, String>>,
) -> Result<(), ExecutionError> {
    for (source, dest) in template_map {
        let source_path = config_dir.join(source);
        let dest_path = PathBuf::from(interpolate_env_vars(dest));

        tracing::info!(
            "Rendering template {} to {}",
            source_path.display(),
            dest_path.display()
        );

        let template_content = fs::read_to_string(&source_path)?;
        let mut rendered = template_content.clone();

        rendered = interpolate_env_vars(&rendered);

        if let Some(vars) = variable_map {
            for (key, value) in vars {
                let pattern = format!("${{{}}}", key);
                rendered = rendered.replace(&pattern, value);
            }
        }

        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&dest_path, rendered)?;
    }

    Ok(())
}

/// Execute a shell command
pub fn execute_command(command: &str) -> Result<(), ExecutionError> {
    tracing::info!("Executing command: {}", command);

    let output = Command::new("sh").arg("-c").arg(command).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(ExecutionError::CommandFailed {
            command: command.to_string(),
            exit_code: output.status.code(),
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        tracing::debug!("{}", stdout);
    }

    Ok(())
}

/// Execute commands for a specific stage
pub fn execute_commands_for_stage(
    commands: &HashMap<CommandStage, Vec<String>>,
    stage: CommandStage,
) -> Result<(), ExecutionError> {
    if let Some(cmds) = commands.get(&stage) {
        tracing::info!("Executing {:?} commands", stage);
        for cmd in cmds {
            execute_command(cmd)?;
        }
    }
    Ok(())
}

/// Perform a service action using systemctl
pub fn perform_service_action(service: &str, action: &ServiceAction) -> Result<(), ExecutionError> {
    let action_str = match action {
        ServiceAction::Stop => "stop",
        ServiceAction::Start => "start",
        ServiceAction::Restart => "restart",
        ServiceAction::Enable => "enable",
        ServiceAction::Disable => "disable",
        ServiceAction::EnableNow => "enable --now",
        ServiceAction::DisableNow => "disable --now",
        ServiceAction::Status => "status",
    };

    tracing::info!("Performing {} on service {}", action_str, service);

    let mut cmd = Command::new("sudo");
    cmd.arg("systemctl");

    for part in action_str.split_whitespace() {
        cmd.arg(part);
    }
    cmd.arg(service);

    let output = cmd.output()?;

    if !output.status.success() && !matches!(action, ServiceAction::Status) {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(ExecutionError::ServiceActionFailed {
            service: service.to_string(),
            action: action_str.to_string(),
            error: stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        tracing::debug!("{}", stdout);
    }

    Ok(())
}

/// Perform all service actions
pub fn perform_service_actions(
    services: &HashMap<String, ServiceAction>,
) -> Result<(), ExecutionError> {
    for (service, action) in services {
        perform_service_action(service, action)?;
    }
    Ok(())
}

/// Execute a complete configuration
pub fn execute_configuration(
    config: &Configuration,
    config_dir: &PathBuf,
) -> Result<bool, ExecutionError> {
    tracing::info!("Executing configuration: {}", config.desc);

    // Stage 1: PreDeps commands
    if let Some(ref commands) = config.commands {
        execute_commands_for_stage(commands, CommandStage::PreDeps)?;
    }

    // Stage 2: Install dependencies
    if let Some(ref deps) = config.dependency_map {
        install_dependencies(deps)?;
    }

    // Stage 3: PostDeps commands
    if let Some(ref commands) = config.commands {
        execute_commands_for_stage(commands, CommandStage::PostDeps)?;
    }

    // Stage 4: PreFiles commands
    if let Some(ref commands) = config.commands {
        execute_commands_for_stage(commands, CommandStage::PreFiles)?;
    }

    // Stage 5: Copy files and render templates
    if let Some(ref file_map) = config.file_map {
        copy_files(config_dir, file_map, &config.variable_map)?;
    }

    if let Some(ref template_map) = config.template_map {
        render_templates(config_dir, template_map, &config.variable_map)?;
    }

    // Stage 6: PostFiles commands
    if let Some(ref commands) = config.commands {
        execute_commands_for_stage(commands, CommandStage::PostFiles)?;
    }

    // Stage 7: Service actions
    if let Some(ref services) = config.services {
        perform_service_actions(services)?;
    }

    // Stage 8: After commands
    if let Some(ref commands) = config.commands {
        execute_commands_for_stage(commands, CommandStage::After)?;
    }

    // Stage 9: Determine if reboot is needed
    let needs_reboot = match config.reboot {
        RebootStrategy::Always => true,
        RebootStrategy::IfRequested => PathBuf::from("/var/run/reboot-required").exists(),
        RebootStrategy::No => false,
    };

    if needs_reboot {
        tracing::warn!("Configuration requires reboot");
    }

    tracing::info!("Configuration execution completed successfully");
    Ok(needs_reboot)
}
