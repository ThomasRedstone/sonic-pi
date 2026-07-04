//! Buffer persistence — workspace files on disk, Sonic Pi style.
//!
//! Lives under `~/.sonic-pi/store/sonic-oxide/buffer_<n>.spi` (its own subdir
//! so it can never clobber the Qt app's `workspace_*` files, while sharing
//! the familiar `~/.sonic-pi` home). Load returns `None` per missing file so
//! callers can fall back to seed content.

use std::path::{Path, PathBuf};

pub fn default_store_dir() -> PathBuf {
    // Escape hatch for smoke-testing things like first-run behaviour
    // against a throwaway directory without touching the user's real
    // workspace/prefs.
    if let Some(dir) = std::env::var_os("SONIC_OXIDE_STORE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let dir = home.join(".sonic-pi/store/sonic-oxide");
    // One-time migration from the pre-naming "streamlined" dir.
    let old = home.join(".sonic-pi/store/streamlined");
    if !dir.exists() && old.exists() {
        if std::fs::rename(&old, &dir).is_err() {
            return old; // migration failed — keep using the old home
        }
    }
    dir
}

fn buffer_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("buffer_{index}.spi"))
}

/// Load up to `n` buffers; `None` where no file exists (or it's unreadable).
pub fn load_buffers(dir: &Path, n: usize) -> Vec<Option<String>> {
    (0..n).map(|i| std::fs::read_to_string(buffer_path(dir, i)).ok()).collect()
}

/// Persist every buffer (best-effort; returns how many were written).
pub fn save_buffers(dir: &Path, buffers: &[String]) -> usize {
    if std::fs::create_dir_all(dir).is_err() {
        return 0;
    }
    buffers
        .iter()
        .enumerate()
        .filter(|(i, text)| std::fs::write(buffer_path(dir, *i), text).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_buffers_and_reports_missing_ones() {
        let dir = std::env::temp_dir().join("sonic_spike_store_test");
        let _ = std::fs::remove_dir_all(&dir);

        // Nothing stored yet → all None.
        assert_eq!(load_buffers(&dir, 3), vec![None, None, None]);

        let bufs = vec!["play 60".to_string(), String::new(), "# three".to_string()];
        assert_eq!(save_buffers(&dir, &bufs), 3);

        let loaded = load_buffers(&dir, 4);
        assert_eq!(loaded[0].as_deref(), Some("play 60"));
        assert_eq!(loaded[1].as_deref(), Some(""));
        assert_eq!(loaded[2].as_deref(), Some("# three"));
        assert_eq!(loaded[3], None); // never written

        // Overwrite persists the newest content.
        save_buffers(&dir, &["sleep 1".to_string()]);
        assert_eq!(load_buffers(&dir, 1)[0].as_deref(), Some("sleep 1"));
    }

    #[test]
    fn default_dir_migrates_the_old_streamlined_store() {
        // Point HOME at a scratch dir (no other test in this binary reads
        // HOME) holding a legacy "streamlined" store.
        let fake_home = std::env::temp_dir().join("sonic_oxide_store_home_test");
        let _ = std::fs::remove_dir_all(&fake_home);
        let old = fake_home.join(".sonic-pi/store/streamlined");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("buffer_0.spi"), "legacy").unwrap();

        let real_home = std::env::var_os("HOME");
        // SAFETY: test-only, sequential env access in this binary.
        unsafe { std::env::set_var("HOME", &fake_home); }
        let dir = default_store_dir();
        // Second call: migration already done, path is stable.
        let dir2 = default_store_dir();
        // SAFETY: test-only, sequential env access in this binary.
        unsafe {
            match real_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }

        assert_eq!(dir, fake_home.join(".sonic-pi/store/sonic-oxide"));
        assert_eq!(dir, dir2);
        assert!(!old.exists(), "old dir should have been renamed away");
        assert_eq!(load_buffers(&dir, 1)[0].as_deref(), Some("legacy"));
        let _ = std::fs::remove_dir_all(&fake_home);
    }
}

/// Tiny prefs table (font size, …) beside the buffers — `key = value` lines.
pub fn load_prefs(dir: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("prefs.conf")) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    map
}

pub fn save_prefs(dir: &Path, prefs: &std::collections::HashMap<String, String>) {
    let _ = std::fs::create_dir_all(dir);
    let mut lines: Vec<String> = prefs.iter().map(|(k, v)| format!("{k} = {v}")).collect();
    lines.sort();
    let _ = std::fs::write(dir.join("prefs.conf"), lines.join("\n") + "\n");
}

#[cfg(test)]
mod prefs_tests {
    use super::*;

    #[test]
    fn prefs_round_trip() {
        let dir = std::env::temp_dir().join("sonic_oxide_prefs_test");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_prefs(&dir).is_empty());
        let mut p = std::collections::HashMap::new();
        p.insert("font_size".to_string(), "18".to_string());
        save_prefs(&dir, &p);
        assert_eq!(load_prefs(&dir)["font_size"], "18");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
