// src/snapshot.rs
//
// Persist the task state at last-fetch time to
// ~/.local/share/nvim/todoist-nvim/snapshot.json
//
// This snapshot is the authoritative "before" state for the diff engine.
// If no snapshot exists (first run), sync falls back to treating every
// task in the buffer as new or unknown and warns accordingly.

use crate::models::Snapshot;
use std::fs;
use std::path::PathBuf;

/// Returns the path to the snapshot file, creating parent dirs if needed.
pub fn snapshot_path() -> Result<PathBuf, String> {
    // Prefer XDG_DATA_HOME, fall back to ~/.local/share.
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local").join("share")
        });

    let dir = data_dir.join("nvim").join("todoist-nvim");
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create snapshot directory {}: {}", dir.display(), e))?;

    Ok(dir.join("snapshot.json"))
}

pub fn save(snapshot: &Snapshot) -> Result<(), String> {
    let path = snapshot_path()?;
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|e| format!("Snapshot serialisation error: {}", e))?;
    fs::write(&path, json)
        .map_err(|e| format!("Cannot write snapshot to {}: {}", path.display(), e))?;
    Ok(())
}

pub fn load() -> Result<Option<Snapshot>, String> {
    let path = snapshot_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let json = fs::read_to_string(&path)
        .map_err(|e| format!("Cannot read snapshot {}: {}", path.display(), e))?;
    let snap: Snapshot = serde_json::from_str(&json)
        .map_err(|e| format!("Snapshot parse error: {}", e))?;
    Ok(Some(snap))
}
