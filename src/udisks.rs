// SPDX-License-Identifier: GPL-3.0-only

//! UDisks2 D-Bus interface for device enumeration.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const UDISKS2_DEST: &str = "org.freedesktop.UDisks2";
const UDISKS2_PATH: &str = "/org/freedesktop/UDisks2";

/// Information about a mounted filesystem.
#[derive(Debug, Clone)]
pub struct DriveInfo {
    /// Mount point path.
    pub mount_point: PathBuf,
    /// Filesystem label, if any.
    pub label: Option<String>,
    /// Device path (e.g., /dev/nvme0n1p1).
    pub device: String,
    /// Filesystem type (e.g., ext4, btrfs).
    #[allow(dead_code)]
    pub fs_type: String,
    /// Drive model name, if available.
    #[allow(dead_code)]
    pub model: Option<String>,
    /// Whether this is a removable drive.
    pub removable: bool,
}

impl DriveInfo {
    /// Returns a display name for this drive.
    ///
    /// Uses the label if available, otherwise derives a name from the mount point.
    pub fn display_name(&self) -> String {
        if let Some(label) = &self.label {
            if !label.is_empty() {
                return label.clone();
            }
        }

        match self.mount_point.to_str() {
            Some("/") => "/".to_string(),
            Some(path) if path.starts_with("/home") => "~".to_string(),
            Some(path) => path
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .to_string(),
            None => self.device.clone(),
        }
    }
}

/// Filesystem types to exclude (virtual/pseudo filesystems).
const EXCLUDED_FS_TYPES: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "squashfs",
    "overlay",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "autofs",
    "proc",
    "sysfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "securityfs",
    "pstore",
    "efivarfs",
    "bpf",
    "fusectl",
    "configfs",
    "debugfs",
    "tracefs",
    "hugetlbfs",
    "mqueue",
    "ramfs",
];

/// Enumerates all mounted filesystems via UDisks2.
///
/// Deduplicates by device path, preferring root (/) and /home mounts
/// over subvolume mounts like /var, /srv, etc.
pub fn enumerate_drives() -> Result<Vec<DriveInfo>> {
    let connection = Connection::system()
        .context("failed to connect to system D-Bus")?;

    let objects = get_managed_objects(&connection)?;
    let mut drives = Vec::new();

    for interfaces in objects.values() {
        // Only care about objects with a Filesystem interface
        let Some(fs_props) = interfaces.get("org.freedesktop.UDisks2.Filesystem") else {
            continue;
        };

        // Get mount points
        let mount_points = get_mount_points(fs_props)?;
        if mount_points.is_empty() {
            continue;
        }

        // Get block device properties
        let Some(block_props) = interfaces.get("org.freedesktop.UDisks2.Block") else {
            continue;
        };

        let device = get_string_prop(block_props, "Device")?;
        let label = get_string_prop(block_props, "IdLabel").ok();
        let fs_type = get_string_prop(block_props, "IdType").unwrap_or_default();

        // Skip virtual/pseudo filesystems
        if EXCLUDED_FS_TYPES.iter().any(|&excluded| fs_type == excluded) {
            continue;
        }

        // Get drive info if available
        let (model, removable) = if let Ok(drive_path) = get_object_path_prop(block_props, "Drive") {
            get_drive_info(&objects, &drive_path).unwrap_or((None, false))
        } else {
            (None, false)
        };

        // Create a DriveInfo for each mount point (usually just one)
        for mount_point in mount_points {
            drives.push(DriveInfo {
                mount_point,
                label: label.clone(),
                device: device.clone(),
                fs_type: fs_type.clone(),
                model: model.clone(),
                removable,
            });
        }
    }

    // Deduplicate by device - keep only the preferred mount point per device
    deduplicate_by_device(&mut drives);

    Ok(drives)
}

/// Filters out subvolume mounts, keeping only primary mount points.
///
/// Always keeps / and /home (even if same device). Discards other subvolumes
/// like /var, /srv, etc. that share a device with / or /home.
fn deduplicate_by_device(drives: &mut Vec<DriveInfo>) {
    use std::collections::HashSet;

    // First pass: find devices that have / or /home
    let mut devices_with_primary: HashSet<String> = HashSet::new();
    for drive in drives.iter() {
        let path = drive.mount_point.to_string_lossy();
        if path == "/" || path.starts_with("/home") {
            devices_with_primary.insert(drive.device.clone());
        }
    }

    // Second pass: keep primary mounts, and non-primary only if device has no primary
    drives.retain(|drive| {
        let path = drive.mount_point.to_string_lossy();
        let is_primary = path == "/" || path.starts_with("/home");

        if is_primary {
            // Always keep / and /home
            true
        } else if devices_with_primary.contains(&drive.device) {
            // This device has / or /home, skip other subvolumes
            false
        } else {
            // Device has no primary mount, keep this one
            true
        }
    });
}

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn get_managed_objects(connection: &Connection) -> Result<ManagedObjects> {
    let reply = connection
        .call_method(
            Some(UDISKS2_DEST),
            UDISKS2_PATH,
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .context("failed to call GetManagedObjects")?;

    reply.body().deserialize().context("failed to deserialize managed objects")
}

fn get_mount_points(fs_props: &HashMap<String, OwnedValue>) -> Result<Vec<PathBuf>> {
    let Some(value) = fs_props.get("MountPoints") else {
        return Ok(Vec::new());
    };

    // MountPoints is an array of byte arrays (each is a null-terminated path)
    let mount_points: Vec<Vec<u8>> = value
        .downcast_ref::<Value>()
        .ok()
        .and_then(|v| match v {
            Value::Array(arr) => {
                let mut result = Vec::new();
                for item in arr.iter() {
                    if let Value::Array(bytes) = item {
                        let byte_vec: Vec<u8> = bytes
                            .iter()
                            .filter_map(|b| match b {
                                Value::U8(byte) => Some(*byte),
                                _ => None,
                            })
                            .collect();
                        result.push(byte_vec);
                    }
                }
                Some(result)
            }
            _ => None,
        })
        .unwrap_or_default();

    let paths = mount_points
        .into_iter()
        .filter_map(|bytes| {
            // Remove null terminator if present
            let bytes = if bytes.last() == Some(&0) {
                &bytes[..bytes.len() - 1]
            } else {
                &bytes[..]
            };
            String::from_utf8(bytes.to_vec())
                .ok()
                .map(PathBuf::from)
        })
        .collect();

    Ok(paths)
}

fn get_string_prop(props: &HashMap<String, OwnedValue>, key: &str) -> Result<String> {
    let value = props
        .get(key)
        .context(format!("missing property: {key}"))?;

    // Device paths come as byte arrays
    if key == "Device" {
        if let Some(bytes) = value.downcast_ref::<Value>().ok().and_then(|v| match v {
            Value::Array(arr) => {
                let byte_vec: Vec<u8> = arr
                    .iter()
                    .filter_map(|b| match b {
                        Value::U8(byte) => Some(*byte),
                        _ => None,
                    })
                    .collect();
                Some(byte_vec)
            }
            _ => None,
        }) {
            let bytes = if bytes.last() == Some(&0) {
                &bytes[..bytes.len() - 1]
            } else {
                &bytes[..]
            };
            return String::from_utf8(bytes.to_vec()).context("invalid UTF-8 in device path");
        }
    }

    // Try as string
    value
        .downcast_ref::<&str>()
        .ok()
        .map(|s| s.to_string())
        .context(format!("property {key} is not a string"))
}

fn get_object_path_prop(props: &HashMap<String, OwnedValue>, key: &str) -> Result<OwnedObjectPath> {
    let value = props
        .get(key)
        .context(format!("missing property: {key}"))?;

    value
        .downcast_ref::<ObjectPath>()
        .ok()
        .map(|p| p.to_owned().into())
        .context(format!("property {key} is not an object path"))
}

fn get_drive_info(
    objects: &ManagedObjects,
    drive_path: &OwnedObjectPath,
) -> Result<(Option<String>, bool)> {
    let interfaces = objects
        .get(drive_path)
        .context("drive object not found")?;

    let drive_props = interfaces
        .get("org.freedesktop.UDisks2.Drive")
        .context("no Drive interface")?;

    let model = drive_props
        .get("Model")
        .and_then(|v| v.downcast_ref::<&str>().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    let removable = drive_props
        .get("Removable")
        .and_then(|v| v.downcast_ref::<bool>().ok())
        .unwrap_or(false);

    Ok((model, removable))
}
