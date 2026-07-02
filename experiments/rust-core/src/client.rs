//! Client-facing types — the Rust mirror of the C++ `IAPIClient` callbacks and
//! their data structs (sonicpi_api.h).
//!
//! The C++ side is a fat interface of ~18 virtual methods (`Report`, `Status`,
//! `Cue`, `Midi`, `Version`, …). In Rust we collapse that into one `on_event`
//! taking a `ClientEvent` enum — same information, but exhaustively matchable
//! and cheaper to implement for non-GUI consumers.

/// Kind of a log/report message (mirrors `MessageType`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageType {
    StartupError,
    RuntimeError,
    SyntaxError,
    Message,
    Info,
    InfoText,
    Multi,
}

/// One line within a multi-message (mirrors `MessageData`).
#[derive(Debug, Clone, Default)]
pub struct MessageData {
    pub text: String,
    pub style: i32,
}

/// A log/error/report (mirrors `MessageInfo`).
#[derive(Debug, Clone)]
pub struct MessageInfo {
    pub kind: MessageType,
    pub text: String,
    pub style: i32,
    pub job_id: i32,
    pub thread_name: String,
    pub runtime: String,
    pub backtrace: String,
    pub line: i32,
    pub multi: Vec<MessageData>,
}

impl MessageInfo {
    pub fn simple(kind: MessageType, text: impl Into<String>) -> Self {
        MessageInfo {
            kind,
            text: text.into(),
            style: 0,
            job_id: 0,
            thread_name: String::new(),
            runtime: String::new(),
            backtrace: String::new(),
            line: 0,
            multi: Vec::new(),
        }
    }
}

/// An incoming OSC cue (mirrors `CueInfo`).
#[derive(Debug, Clone)]
pub struct CueInfo {
    pub time: String,
    pub address: String,
    pub id: i32,
    pub args: String,
    pub index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiType {
    Out,
    In,
}

#[derive(Debug, Clone)]
pub struct MidiInfo {
    pub kind: MidiType,
    pub port_info: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusType {
    Ack,
    AllComplete,
    Exited,
}

#[derive(Debug, Clone)]
pub struct StatusInfo {
    pub kind: StatusType,
    pub id: String,
}

#[derive(Debug, Clone, Default)]
pub struct VersionInfo {
    pub version: String,
    pub num: i32,
    pub latest_version: String,
    pub latest_num: i32,
    pub platform: String,
}

/// Audio output devices pushed by the engine (`/supersonic/devices`).
/// Mirrors `AudioDevicesInfo`.
#[derive(Debug, Clone, Default)]
pub struct AudioDevicesInfo {
    pub mode: String,
    pub current_device: String,
    pub devices: Vec<String>,
    pub sample_rate: i32,
}

/// Audio input devices pushed by the engine (`/supersonic/input-devices`).
/// Mirrors `AudioInputDevicesInfo`.
#[derive(Debug, Clone, Default)]
pub struct AudioInputDevicesInfo {
    pub current_device: String,
    pub devices: Vec<String>,
}

/// Engine status text (`/supersonic/info`) — simple (text-only) or the
/// extended form's leading fields. Mirrors `ScsynthInfo`.
#[derive(Debug, Clone, Default)]
pub struct ScsynthInfo {
    pub text: String,
    pub sample_rate: Option<i32>,
    pub buffer_size: Option<i32>,
}

/// Events decoded from incoming OSC — one variant per `IAPIClient` callback.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Report(MessageInfo),
    Status(StatusInfo),
    Cue(CueInfo),
    Midi(MidiInfo),
    Version(VersionInfo),
    Bpm(f64),
    ActiveLinks(i32),
    SpiderReady,
    GamepadDevices(String),
    AudioDevices(AudioDevicesInfo),
    AudioInputDevices(AudioInputDevicesInfo),
    Scsynth(ScsynthInfo),
    /// Boot failed before the runtime came up (`/exited-with-boot-error`).
    BootError(String),
    /// A recognised address we haven't modelled yet (scaffold TODO). Carries
    /// the raw address so the gap is visible rather than silently dropped.
    Unhandled { addr: String },
}

/// The callback surface — implement to receive engine events. Mirrors
/// `IAPIClient`. Called from the OSC server thread, so implementations must be
/// `Send + Sync` and do their own UI-thread marshalling.
pub trait ApiClient: Send + Sync {
    fn on_event(&self, event: ClientEvent);
}
