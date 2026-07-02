//! Sonic Pi core — Phase 2 scaffold.
//!
//! A Rust reimplementation of the C++ `app/api` layer (`SonicPiAPI`), designed
//! to sit behind the *same* contract so it can be validated with the existing
//! Qt GUI as an oracle (see ../../plan/02-implementation-plan.md, Phase 2).
//!
//! The contract, reverse-engineered from the C++ headers/sources in this repo:
//!   * `app/api/include/api/sonicpi_api.h`  — the API surface + `IAPIClient`
//!   * `app/api/src/sonicpi_api.cpp`        — outgoing OSC + daemon handshake
//!   * `app/api/src/osc/osc_handler.cpp`    — incoming OSC → client callbacks
//!
//! Process model (unchanged): a Ruby **daemon** is spawned; it prints a
//! handshake line of ports + a token, then supervises the **Spider** runtime
//! and **SuperSonic** engine. This core talks OSC to all three and tails the
//! engine's shared-memory ring for the scope (proven separately in
//! `experiments/gpui-spike/src/shm.rs`).
//!
//! Status: scaffold. The OSC vocabulary, handshake parsing, port/sender wiring,
//! and event dispatch are real and tested; process boot and audio are skeletons
//! with clear TODOs (they need the full runtime to exercise).

pub mod audio;
pub mod client;
pub mod osc;
pub mod paths;
pub mod ports;
pub mod process;
pub mod protocol;

mod session;

// Re-export rosc so consumers build/inspect OSC messages against our exact
// version (no separate dependency, no version skew).
pub use rosc;

pub use client::{
    ApiClient, AudioDevicesInfo, AudioInputDevicesInfo, ClientEvent, CueInfo, MessageData,
    MessageInfo, MessageType, MidiInfo, MidiType, ScsynthInfo, StatusInfo, StatusType,
    VersionInfo,
};
pub use ports::{PortId, Ports};
pub use session::{KeepAlive, Session};

/// Errors surfaced by the core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("osc error: {0}")]
    Osc(#[from] rosc::OscError),
    #[error("daemon handshake: {0}")]
    Handshake(String),
    #[error("process spawn: {0}")]
    Spawn(String),
}
