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

    // The bundled Ruby location is platform-specific; use system `ruby` as a
    // stand-in until the bundled-runtime resolution is ported.
    m.insert(SonicPiPath::Ruby, PathBuf::from("ruby"));
    m
}
