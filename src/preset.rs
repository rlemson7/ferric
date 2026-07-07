//! User preset I/O. Presets are JSON files (`.ferric` extension) holding a
//! map of parameter id → **plain** (unnormalized) value as a string, e.g.
//! `"time": "380"`, `"feedback": "0.55"`, `"mode": "2"` (enum variant
//! index), `"reverse": "1"` (bool). Human-readable and hand-editable.
//!
//! NIH-plug's `Params::serialize_fields` only covers `#[persist]` fields —
//! not parameters — so saving reads each parameter's plain value through
//! `param_map()` instead. Loading happens in the editor: plain values are
//! converted back with `ParamPtr::preview_normalized` and applied through
//! the GUI's `RawParamEvent`s so the host sees proper parameter changes
//! (see `apply_preset` / `apply_defaults` in `editor.rs`).

use crate::FerricParams;
use nih_plug::prelude::Params;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const EXT: &str = "ferric";

/// Parameter ids that never belong in a preset. GUI scale is a per-user
/// window setting, not part of a sound.
pub const SKIP_IDS: &[&str] = &["scale"];

/// User preset folder. macOS prefers the standard `~/Library/Audio/Presets/`
/// vendor/plugin layout so DAWs that read system preset folders find them —
/// but on some systems that folder is root-owned and not user-writable, so
/// it's only used if the plugin can actually create its subfolder there.
/// Everything else (including that case) falls back to the platform's local
/// data dir (`~/Library/Application Support/FERRIC/Presets` on macOS).
pub fn preset_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        let standard = home
            .join("Library")
            .join("Audio")
            .join("Presets")
            .join("Realtime Media")
            .join("FERRIC");
        if fs::create_dir_all(&standard).is_ok() {
            return standard;
        }
    }
    if let Some(d) = dirs::data_local_dir() {
        return d.join("FERRIC").join("Presets");
    }
    PathBuf::from("./presets")
}

/// Save the current parameter state (plain values) to `path`. Adds the
/// `.ferric` extension if missing.
pub fn save_preset(path: &Path, params: &FerricParams) -> Result<(), String> {
    let path = if path.extension().is_none() {
        path.with_extension(EXT)
    } else {
        path.to_path_buf()
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (id, ptr, _group) in params.param_map() {
        if SKIP_IDS.contains(&id.as_str()) {
            continue;
        }
        // Safety: `ptr` points into `params`, which outlives this call.
        let plain = unsafe { ptr.unmodulated_plain_value() };
        map.insert(id, format!("{plain}"));
    }
    let json = serde_json::to_string_pretty(&map).map_err(|e| format!("json: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write: {e}"))
}

/// Read a preset file into its id → plain-value map. Application to the
/// live parameters is the editor's job (it needs a GUI event context).
pub fn read_preset(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let s = fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("parse: {e}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset shipped in the repo's `presets/` folder must reference
    /// only real parameter ids, with finite numeric values that survive the
    /// plain → normalized → plain round trip (i.e. they're inside the
    /// parameter's range — a clamped value would come back different).
    #[test]
    fn shipped_presets_are_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("presets");
        let params = FerricParams::default();
        let ptrs: BTreeMap<String, nih_plug::prelude::ParamPtr> = params
            .param_map()
            .into_iter()
            .map(|(id, ptr, _)| (id, ptr))
            .collect();

        let files = fs::read_dir(&dir)
            .expect("presets/ folder missing")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some(EXT))
            .collect::<Vec<_>>();
        assert!(
            files.len() >= 10,
            "expected a preset library, found {} files",
            files.len()
        );

        for file in files {
            let name = file.display().to_string();
            let map = read_preset(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!map.is_empty(), "{name}: empty preset");
            for (id, val) in &map {
                assert!(
                    !SKIP_IDS.contains(&id.as_str()),
                    "{name}: contains skipped id {id}"
                );
                let ptr = ptrs
                    .get(id)
                    .unwrap_or_else(|| panic!("{name}: unknown param id {id}"));
                let plain: f32 = val
                    .parse()
                    .unwrap_or_else(|_| panic!("{name}: non-numeric {id} = {val}"));
                assert!(plain.is_finite(), "{name}: non-finite {id}");
                let (n, back) = unsafe {
                    let n = ptr.preview_normalized(plain);
                    (n, ptr.preview_plain(n))
                };
                assert!(
                    (0.0..=1.0).contains(&n),
                    "{name}: {id} = {val} normalizes outside [0,1]"
                );
                assert!(
                    (back - plain).abs() <= 0.01 * plain.abs().max(1.0),
                    "{name}: {id} = {val} is outside the param range (round-trips to {back})"
                );
            }
        }
    }

    /// `save_preset` writes plain values for every param except the skipped
    /// ones, and `read_preset` round-trips the file.
    #[test]
    fn save_and_read_round_trip() {
        let params = FerricParams::default();
        let dir = std::env::temp_dir().join("ferric_preset_test");
        let path = dir.join("roundtrip.ferric");
        save_preset(&path, &params).expect("save");
        let map = read_preset(&path).expect("read");
        assert!(map.contains_key("time"));
        assert!(map.contains_key("feedback"));
        assert!(!map.contains_key("scale"));
        // Default time is 350 ms, stored as a plain value.
        let time: f32 = map["time"].parse().unwrap();
        assert!((time - 350.0).abs() < 1e-3);
        let _ = fs::remove_dir_all(&dir);
    }
}
