//! Buffer persistence — workspace files on disk, Sonic Pi style.
//!
//! Lives under `~/.sonic-pi/store/sonic-oxide/buffer_<n>.spi` (its own subdir
//! so it can never clobber the Qt app's `workspace_*` files, while sharing
//! the familiar `~/.sonic-pi` home). Load returns `None` per missing file so
//! callers can fall back to seed content.

use std::path::{Path, PathBuf};

pub fn default_store_dir() -> PathBuf {
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
}
