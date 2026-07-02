//! Path resolution — Rust mirror of `SonicPiPath` + `InitializePaths`.
//!
//! Scaffold: the joins below reflect the current repo layout under `app/`. The
//! real C++ also resolves the user-writable home/config/log dirs; those are
//! left as TODO here since they need the platform user-dir logic (which maps to
//! the `directories` crate in the eventual core).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SonicPiPath {
    Root,
    Ruby,
    BootDaemon,
    Samples,
    ServerBin,
}

/// Resolve the application paths relative to the Sonic Pi root (the repo's
/// `app/` dir, or an install root). Mirrors the server-relative joins the C++
/// performs; user/home/config/log dirs are TODO.
pub fn resolve(root: &Path) -> HashMap<SonicPiPath, PathBuf> {
    let server = root.join("server");
    let ruby_bin = server.join("ruby").join("bin");

    let mut m = HashMap::new();
    m.insert(SonicPiPath::Root, root.to_path_buf());
    m.insert(SonicPiPath::ServerBin, ruby_bin.clone());
    m.insert(SonicPiPath::BootDaemon, ruby_bin.join("daemon.rb"));
    m.insert(SonicPiPath::Samples, root.join("..").join("etc").join("samples"));

    // Ruby resolution, most specific first: explicit override, the bundled
    // runtime (official Sonic Pi layout: server/native/ruby/bin/ruby), then
    // the system interpreter.
    let ruby = std::env::var_os("SONIC_OXIDE_RUBY")
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_else(|| {
            let bundled = server.join("native").join("ruby").join("bin").join("ruby");
            if bundled.exists() {
                bundled
            } else {
                PathBuf::from("ruby")
            }
        });
    m.insert(SonicPiPath::Ruby, ruby);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_bundled_ruby_when_present() {
        let root = std::env::temp_dir().join("sonic_oxide_paths_test");
        let bundled_bin = root.join("server/native/ruby/bin");
        std::fs::create_dir_all(&bundled_bin).unwrap();
        let bundled = bundled_bin.join("ruby");
        std::fs::write(&bundled, "#!/bin/sh\n").unwrap();

        let paths = resolve(&root);
        assert_eq!(paths[&SonicPiPath::Ruby], bundled);

        // No bundle → system fallback.
        std::fs::remove_file(&bundled).unwrap();
        let paths = resolve(&root);
        assert_eq!(paths[&SonicPiPath::Ruby], PathBuf::from("ruby"));
    }
}
