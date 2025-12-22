// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use cosmic::cosmic_config::{self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry};

/// Per-drive alert configuration.
#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DriveAlertConfig {
    /// Whether alerts are enabled for this drive.
    pub enabled: bool,
    /// Usage percentage at which to trigger alerts.
    pub threshold: u8,
}

impl Default for DriveAlertConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 90,
        }
    }
}

/// Applet configuration stored via cosmic-config.
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// Seconds between disk space checks.
    pub poll_interval: u64,
    /// Default usage percentage at which to trigger alerts (for drives without custom settings).
    pub default_alert_threshold: u8,
    /// Mount points to monitor. Empty means auto-detect all persistent drives.
    pub monitored_drives: Vec<String>,
    /// Seconds before re-alerting for the same drive.
    pub alert_cooldown: u64,
    /// Mount points to display on the panel.
    pub panel_drives: Vec<String>,
    /// Per-drive alert settings. Key is mount point path.
    pub drive_alerts: HashMap<String, DriveAlertConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval: 30,
            default_alert_threshold: 90,
            monitored_drives: Vec::new(),
            alert_cooldown: 3600,
            panel_drives: vec!["/".to_string(), "/home".to_string()],
            drive_alerts: HashMap::new(),
        }
    }
}

impl Config {
    /// Gets alert config for a drive, returning default if not set.
    pub fn get_drive_alert(&self, mount_point: &str) -> DriveAlertConfig {
        self.drive_alerts
            .get(mount_point)
            .cloned()
            .unwrap_or(DriveAlertConfig {
                enabled: true,
                threshold: self.default_alert_threshold,
            })
    }
}
