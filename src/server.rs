use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;
use std::io;
use serde::{Deserialize, Serialize};
use uuid7::Uuid;
use git2::{Repository, Cred, RemoteCallbacks, FetchOptions, build::RepoBuilder};

use crate::common::*;

// Server Configuration

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub id: Uuid,
    pub bind_address: String,
    pub git_repos: Vec<GitRepository>,
    pub sync_interval_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            id: uuid7::uuid7(),
            bind_address: "0.0.0.0:8080".to_string(),
            git_repos: vec![],
            sync_interval_secs: 300,
        }
    }
}

// Git Operations

#[derive(Debug)]
pub enum GitError {
    Git(git2::Error),
    Io(io::Error),
}

impl From<git2::Error> for GitError {
    fn from(e: git2::Error) -> Self {
        GitError::Git(e)
    }
}

impl From<io::Error> for GitError {
    fn from(e: io::Error) -> Self {
        GitError::Io(e)
    }
}

/// Clone or update a git repository
pub fn sync_git_repository(
    repo_config: &GitRepository,
    local_path: &PathBuf,
) -> Result<String, GitError> {
    if local_path.exists() {
        // Repository exists, fetch and update
        let repo = Repository::open(local_path)?;
        let mut remote = repo.find_remote("origin")?;

        let mut callbacks = RemoteCallbacks::new();
        if let Some(ssh_key) = &repo_config.ssh_key_path {
            callbacks.credentials(|_url, username_from_url, _allowed_types| {
                Cred::ssh_key(
                    username_from_url.unwrap_or("git"),
                    None,
                    std::path::Path::new(ssh_key),
                    None,
                )
            });
        }

        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(callbacks);

        remote.fetch(&[&repo_config.branch], Some(&mut fetch_options), None)?;

        // Get the latest commit hash
        let fetch_head = repo.find_reference("FETCH_HEAD")?;
        let fetch_commit = repo.reference_to_annotated_commit(&fetch_head)?;
        let commit = repo.find_commit(fetch_commit.id())?;

        // Fast-forward merge
        let refname = format!("refs/heads/{}", repo_config.branch);
        let mut reference = repo.find_reference(&refname)?;
        reference.set_target(commit.id(), "Fast-forward")?;
        repo.set_head(&refname)?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;

        Ok(commit.id().to_string())
    } else {
        // Clone repository
        let mut builder = RepoBuilder::new();

        let mut callbacks = RemoteCallbacks::new();
        if let Some(ssh_key) = &repo_config.ssh_key_path {
            callbacks.credentials(|_url, username_from_url, _allowed_types| {
                Cred::ssh_key(
                    username_from_url.unwrap_or("git"),
                    None,
                    std::path::Path::new(ssh_key),
                    None,
                )
            });
        }

        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(callbacks);
        builder.fetch_options(fetch_options);
        builder.branch(&repo_config.branch);

        let repo = builder.clone(&repo_config.url, local_path)?;
        let head = repo.head()?;
        let commit = head.peel_to_commit()?;

        Ok(commit.id().to_string())
    }
}

// Repository Parsing

/// Load all configurations from a git repository
pub fn load_configurations_from_repo(repo_path: &PathBuf) -> io::Result<HashMap<String, Configuration>> {
    let mut configs = HashMap::new();
    let configs_dir = repo_path.join("configurations");

    if !configs_dir.exists() {
        return Ok(configs);
    }

    for entry in fs::read_dir(&configs_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let config_file = path.join("config.toml");
            if config_file.exists() {
                let config_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid directory name"))?;

                match load_configuration(&config_file) {
                    Ok(config) => {
                        configs.insert(config_name.to_string(), config);
                    }
                    Err(e) => tracing::error!("Failed to load configuration {}: {}", config_name, e),
                }
            }
        }
    }

    Ok(configs)
}

/// Load all compositions from a git repository
pub fn load_compositions_from_repo(repo_path: &PathBuf) -> io::Result<HashMap<String, Composition>> {
    let mut compositions = HashMap::new();
    let compositions_dir = repo_path.join("compositions");

    if !compositions_dir.exists() {
        return Ok(compositions);
    }

    for entry in fs::read_dir(&compositions_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            let composition_name = path.file_stem()
                .and_then(|n| n.to_str())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid file name"))?;

            match load_composition(&path) {
                Ok(composition) => {
                    compositions.insert(composition_name.to_string(), composition);
                }
                Err(e) => tracing::error!("Failed to load composition {}: {}", composition_name, e),
            }
        }
    }

    Ok(compositions)
}

/// Load all personas from a git repository
pub fn load_personas_from_repo(repo_path: &PathBuf) -> io::Result<HashMap<String, Persona>> {
    let mut personas = HashMap::new();
    let personas_dir = repo_path.join("personas");

    if !personas_dir.exists() {
        return Ok(personas);
    }

    for entry in fs::read_dir(&personas_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            let persona_name = path.file_stem()
                .and_then(|n| n.to_str())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid file name"))?;

            match load_persona(&path) {
                Ok(persona) => {
                    personas.insert(persona_name.to_string(), persona);
                }
                Err(e) => tracing::error!("Failed to load persona {}: {}", persona_name, e),
            }
        }
    }

    Ok(personas)
}
