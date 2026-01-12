use serde::{Deserialize, Serialize};
use uuid7::Uuid;

// Bridge Configuration

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    pub id: Uuid,
    pub server_url: String,
    pub mqtt_broker: String,
    pub mqtt_port: u16,
    pub sync_interval_secs: u64,
}

// TODO: Add MQTT bridge functionality
// - Connect to MQTT broker
// - Subscribe to agent topics
// - Publish configuration updates
// - Relay agent status to server
