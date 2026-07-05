//! User preset I/O. Presets are JSON files (`.ferric` extension) holding
//! the serialized form of every host-automatable parameter.

use crate::FerricParams;
use nih_plug::prelude::Params;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const EXT: &str = "ferric";

/// User preset folder. macOS uses the standard `~/Library/Audio/Presets/`
/// vendor/plugin layout so DAWs that read system preset folders find them.
/// Other OSes fall back to the platform's local data dir.
pub fn preset_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        return home
            .join("Library")
            .join("Audio")
            .join("Presets")
            .join("Realtime Media")
            .join("FERRIC");
    }
    if let Some(d) = dirs::data_local_dir() {
        return d.join("FERRIC").join("Presets");
    }
    PathBuf::from("./presets")
}

/// Save the current parameter state to `path`. Adds the `.ferric` extension
/// if missing.
pub fn save_preset(path: &Path, params: &FerricParams) -> Result<(), String> {
    let path = if path.extension().is_none() {
        path.with_extension(EXT)
    } else {
        path.to_path_buf()
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let map: BTreeMap<String, String> = params.serialize_fields();
    let json = serde_json::to_string_pretty(&map).map_err(|e| format!("json: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write: {e}"))
}

/// Load `path` and apply it onto `params`. Uses `Params::deserialize_fields`,
/// which writes values via interior mutability without going through host
/// param-change events. Most hosts re-query on next access; if you have
/// active automation lanes they'll continue to override.
pub fn load_preset(path: &Path, params: &FerricParams) -> Result<(), String> {
    let s = fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let map: BTreeMap<String, String> =
        serde_json::from_str(&s).map_err(|e| format!("parse: {e}"))?;
    params.deserialize_fields(&map);
    Ok(())
}

/// Reset every param to its declared default. Uses `deserialize_fields`
/// because `ParamPtr::set_normalized_value` is `pub(crate)` in NIH-plug —
/// generating a fresh `FerricParams::default()` and serializing its fields
/// gives us a complete defaults map without poking internals.
pub fn reset_to_defaults(params: &FerricParams) {
    let defaults = FerricParams::default().serialize_fields();
    params.deserialize_fields(&defaults);
}

/// Discover all `.ferric` preset files in the user preset folder, sorted
/// by name.
pub fn list_presets() -> Vec<PathBuf> {
    let dir = preset_dir();
    let mut out: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(it) => it
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some(EXT))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

pub fn preset_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("(unnamed)")
        .to_string()
}
