# Devendra

`Less heavy than an ansible, orinico flow`


## Features

- Lightweight and easy to use
- Supports multiple platforms
- Composable Machine Configuration
- Limited Scope
- No YAML

## Usage

- Define machine configurations in a simple, declarative format
- Apply configurations to target machines
- Monitor and manage machine states
- Integrate with existing infrastructure tools

## Parts

- Agent
  - Our Rust Agent, In charge of applying personas 
- Server
  - Communicates with the git database and publishes persona changes
- Bridge
  - Handles communication between the agent and server