//! Sonic Oxide — GPUI frontend (Phase 3a build-out).
//!
//! What this now does (see ../../plan/02-implementation-plan.md):
//!   * Multi-buffer editor (3 buffers, tab row) + Cues + Log + Scope panes.
//!   * "Run ▶" sends the active buffer through `sonicpi-core` as
//!     `/save-and-run-buffer`; "Stop ■" sends `/stop-all-jobs`.
//!   * On startup it tries to boot the REAL Ruby daemon (handshake → Session).
//!     When the local runtime is incomplete (no SuperSonic binary), it falls
//!     back to an in-process loopback "spider" so the full OSC path still runs.
//!   * Run-flash: the editor border flashes on eval (live-coding affordance).
//!   * Scope reads SuperSonic's shm ring when `shm_writer` is running.
//!
//! Accessibility: panes carry AccessKit roles + labels (Tier-1). The editor
//! additionally goes through `EditorA11y`, a custom element that writes the
//! buffer text into the AccessKit node *value* — the first Tier-2 increment.

mod i18n;
mod store;
mod vocab;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use vocab::{word_prefix_at, Vocab, VocabKind};

// Live-coding keyboard shortcuts (bound in `main`, handled on the root view).
actions!(sonic_spike, [RunBuffer, StopAll, CommentToggle, AlignBuffer, NextBuffer, PrevBuffer]);

/// Header commands — one identity shared by mouse clicks, keyboard actions
/// and screen-reader Click actions.
#[derive(Clone, Copy)]
enum Cmd {
    Run,
    Stop,
    Rec,
    Comment,
    Align,
    ScopePause,
    ScopeMode,
    Settings,
    Help,
    Nodes,
    Debug,
}

use gpui::*;
use gpui_component::{
    ActiveTheme, Root, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    highlighter::{Diagnostic, DiagnosticSeverity},
    input::{Input, InputState, Position},
    resizable::{h_resizable, resizable_panel, v_resizable},
    v_flex,
};
use gpui_component_assets::Assets;

use sonicpi_core::audio::{
    metrics_idx, spectrum, MetricsReader, NodeInfo, NodeTreeReader, ScopeReader, ScopeSlotReader,
    SpectrumFrame, SpectrumProcessor, SCOPE_SHM_NAME,
};
use sonicpi_core::osc::{OscServer, UdpOscSender};
use sonicpi_core::paths::{resolve, SonicPiPath};
use sonicpi_core::ports::PortId;
use sonicpi_core::process::Daemon;
use sonicpi_core::rosc::{OscMessage, OscType};
use sonicpi_core::supervisor::Supervisor;
use sonicpi_core::{
    protocol, ApiClient, AudioDevicesInfo, AudioInputDevicesInfo, ClientEvent, Session, StatusType,
};

const BUFFER_SEEDS: [&str; 3] = [
    r#"# Buffer 1 — drums
live_loop :drums do
  sample :bd_haus, amp: 2
  sleep 0.5
end
"#,
    r#"# Buffer 2 — bass
live_loop :bass do
  use_synth :tb303
  play :e1, release: 0.2, cutoff: 90
  sleep 0.25
end
"#,
    r#"# Buffer 3 — sketch
play_chord [:c4, :e4, :g4]
"#,
];

/// Qt parity: ten workspaces. Buffers beyond the seeds start empty.
const BUFFER_COUNT: usize = 10;

/// Autosave cadence in 33ms ticks (~5s).
const AUTOSAVE_TICKS: u32 = 150;

const CUES_EXAMPLE: &str = "\
/beat        [t=12.500]  16
/midi/note   [t=12.512]  60 100
/link/tempo  [t=12.600]  120.0
/beat        [t=13.000]  17
";

/// Render a decoded core `ClientEvent` as a log line.
/// One rendered log row: colour comes from the run id (cycling palette, like
/// Qt's `SonicPiLog`), errors go red, multi-message members indent under
/// their run header.
#[derive(Clone)]
struct LogLine {
    run: Option<i32>,
    error: bool,
    indent: bool,
    /// Engine-internals noise (unmodelled OSC etc.) — hidden unless the
    /// debug toggle is on (Qt parity: hide/reveal SuperSonic debug logs).
    debug: bool,
    text: String,
}

impl LogLine {
    fn info(text: impl Into<String>) -> LogLine {
        LogLine { run: None, error: false, indent: false, debug: false, text: text.into() }
    }

    fn debug(text: impl Into<String>) -> LogLine {
        LogLine { run: None, error: false, indent: false, debug: true, text: text.into() }
    }
}

/// Convert one engine event into zero or more log rows (cues go to the Cues
/// pane instead — see the tick loop).
fn log_lines_for(ev: &ClientEvent) -> Vec<LogLine> {
    use sonicpi_core::MessageType as MT;
    match ev {
        ClientEvent::Report(m) if !m.multi.is_empty() => {
            let mut rows = vec![LogLine {
                run: Some(m.job_id),
                error: false,
                indent: false,
                debug: false,
                text: format!("{{run {}}} {}", m.job_id, m.thread_name),
            }];
            rows.extend(m.multi.iter().map(|d| LogLine {
                run: Some(m.job_id),
                error: false,
                indent: true,
                debug: false,
                text: d.text.clone(),
            }));
            rows
        }
        ClientEvent::Report(m) => {
            let error = matches!(m.kind, MT::StartupError | MT::RuntimeError | MT::SyntaxError);
            let run = (m.job_id > 0).then_some(m.job_id);
            let text = if error && !m.runtime.is_empty() {
                format!("[error] {} ({})", m.text, m.runtime)
            } else {
                m.text.clone()
            };
            vec![LogLine { run, error, indent: false, debug: false, text }]
        }
        ClientEvent::Status(s) => vec![LogLine::info(format!("[{:?}] {}", s.kind, s.id))],
        ClientEvent::SpiderReady => vec![LogLine::info("[spider ready]")],
        ClientEvent::Bpm(b) => vec![LogLine::info(format!("[bpm] {b}"))],
        ClientEvent::AudioDevices(d) => vec![LogLine::info(format!(
            "[audio] {} output device(s), current: {} @ {}Hz",
            d.devices.len(),
            d.current_device,
            d.sample_rate
        ))],
        ClientEvent::AudioInputDevices(d) => vec![LogLine::info(format!(
            "[audio] {} input device(s), current: {}",
            d.devices.len(),
            d.current_device
        ))],
        ClientEvent::Scsynth(i) => vec![LogLine::info(format!("[scsynth] {}", i.text))],
        ClientEvent::Cue(_) => vec![], // rendered in the Cues pane
        ClientEvent::Unhandled { addr } => vec![LogLine::debug(format!("[osc] {addr}"))],
        other => vec![LogLine::debug(format!("{other:?}"))],
    }
}

/// Per-run colour cycle (mirrors SonicPiLog's rotating run colours).
const RUN_PALETTE: [u32; 6] = [0x61afef, 0xc678dd, 0xd19a66, 0x98c379, 0x56b6c2, 0xe5c07b];

fn run_color(run: i32) -> Hsla {
    let rgb = RUN_PALETTE[(run.max(0) as usize) % RUN_PALETTE.len()];
    Rgba { r: ((rgb >> 16) & 0xff) as f32 / 255., g: ((rgb >> 8) & 0xff) as f32 / 255., b: (rgb & 0xff) as f32 / 255., a: 1. }.into()
}

fn osc(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage { addr: addr.to_string(), args }
}

/// Locate the Sonic Pi `app/` runtime at RUNTIME (a bundle relocates):
/// `SONIC_OXIDE_APP_ROOT` env, else `../app` next to the executable (the
/// packaged layout), else the dev-checkout fallback baked at compile time.
fn app_root() -> PathBuf {
    if let Some(root) = std::env::var_os("SONIC_OXIDE_APP_ROOT") {
        return PathBuf::from(root);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("../app");
            if bundled.join("server").exists() {
                return bundled;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app")
}

/// Parse Spider's rational cue timestamp (`"num/den"`, in seconds) to f64.
fn parse_cue_time(t: &str) -> Option<f64> {
    let (n, d) = t.split_once('/')?;
    let n: f64 = n.trim().parse().ok()?;
    let d: f64 = d.trim().parse().ok()?;
    (d != 0.0).then(|| n / d)
}

/// Toggle `# ` comments on the lines covered by `sel` (or the cursor's line).
/// Returns the new buffer text, or None if nothing changes. Pure → unit-tested.
fn toggle_comment(text: &str, sel: std::ops::Range<usize>) -> Option<String> {
    // Clamp to char boundaries so non-ASCII buffers can't panic the slicing.
    let clamp = |mut i: usize| {
        i = i.min(text.len());
        while i > 0 && !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (a, b) = (clamp(sel.start), clamp(sel.end.max(sel.start)));
    let start = text[..a].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[b..].find('\n').map(|i| b + i).unwrap_or(text.len());
    let block = &text[start..end];
    if block.trim().is_empty() {
        return None;
    }

    let all_commented = block
        .lines()
        .filter(|l| !l.trim().is_empty())
        .all(|l| l.trim_start().starts_with('#'));

    let new_block = block
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else if all_commented {
                let idx = l.find('#').unwrap();
                let rest = &l[idx + 1..];
                format!("{}{}", &l[..idx], rest.strip_prefix(' ').unwrap_or(rest))
            } else {
                format!("# {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!("{}{}{}", &text[..start], new_block, &text[end..]))
}

/// Re-indent a whole buffer by Sonic Pi / Ruby block structure (2 spaces per
/// level) — the equivalent of the Qt app's "align text". Heuristic, not a
/// parser: block openers are recognised at statement level, `end`/`else`/
/// `elsif`/`when`/`rescue`/`ensure` dedent themselves. Pure → unit-tested.
fn reindent(text: &str) -> String {
    fn opens_block(trimmed: &str) -> bool {
        if trimmed.starts_with('#') {
            return false;
        }
        // `... do` / `... do |x|` block tails.
        if trimmed.ends_with(" do") || trimmed == "do" {
            return true;
        }
        if trimmed.contains(" do |") && trimmed.ends_with('|') {
            return true;
        }
        // Statement-level openers (NOT modifier `play 60 if x`).
        let first = trimmed.split_whitespace().next().unwrap_or("");
        matches!(first, "def" | "if" | "unless" | "while" | "until" | "case" | "begin")
    }
    fn dedents_self(trimmed: &str) -> bool {
        let first = trimmed
            .split(|c: char| c.is_whitespace() || c == '.' || c == ';')
            .next()
            .unwrap_or("");
        matches!(first, "end" | "else" | "elsif" | "when" | "rescue" | "ensure")
    }
    fn closes_block(trimmed: &str) -> bool {
        trimmed == "end" || trimmed.starts_with("end.") || trimmed.starts_with("end ")
    }

    let mut depth: usize = 0;
    let mut out = Vec::new();
    for line in text.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(String::new());
            continue;
        }
        let this_depth = if dedents_self(trimmed) { depth.saturating_sub(1) } else { depth };
        out.push(format!("{}{}", "  ".repeat(this_depth), trimmed));
        if closes_block(trimmed) {
            depth = depth.saturating_sub(1);
        } else if opens_block(trimmed) {
            depth += 1;
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{reindent, toggle_comment};

    #[test]
    fn reindents_do_end_blocks() {
        let messy = "live_loop :drums do\nsample :bd_haus\nif one_in(2)\nplay 60\nelse\nplay 72\nend\n2.times do\nplay 64\nend\nend\n";
        let expect = "live_loop :drums do\n  sample :bd_haus\n  if one_in(2)\n    play 60\n  else\n    play 72\n  end\n  2.times do\n    play 64\n  end\nend\n";
        assert_eq!(reindent(messy), expect);
    }

    #[test]
    fn reindent_ignores_modifier_if_and_comments() {
        let text = "# a comment\nplay 60 if one_in(2)\nsleep 1\n";
        assert_eq!(reindent(text), text); // nothing opens a block
    }

    #[test]
    fn reindent_handles_do_with_block_args() {
        let messy = "with_fx :reverb do |fx|\ncontrol fx, mix: 0.3\nend";
        let expect = "with_fx :reverb do |fx|\n  control fx, mix: 0.3\nend";
        assert_eq!(reindent(messy), expect);
    }

    #[test]
    fn comments_and_uncomments_a_block() {
        let text = "play 60\nsleep 1\n";
        let commented = toggle_comment(text, 0..text.len() - 1).unwrap();
        assert_eq!(commented, "# play 60\n# sleep 1\n");
        let back = toggle_comment(&commented, 0..commented.len() - 1).unwrap();
        assert_eq!(back, text);
    }

    #[test]
    fn single_line_at_cursor() {
        let text = "play 60\nsleep 1\n";
        let out = toggle_comment(text, 10..10).unwrap(); // cursor inside "sleep 1"
        assert_eq!(out, "play 60\n# sleep 1\n");
    }

    #[test]
    fn preserves_indentation_on_uncomment() {
        let text = "  # play 60";
        let out = toggle_comment(text, 0..0).unwrap();
        assert_eq!(out, "  play 60");
    }

    use super::{byte_to_text_position, line_runs, parse_cue_time, word_starts};

    #[test]
    fn parses_rational_cue_timestamps() {
        assert_eq!(parse_cue_time("1783000703/1000"), Some(1_783_000.703));
        assert_eq!(parse_cue_time("3/2"), Some(1.5));
        assert_eq!(parse_cue_time("nonsense"), None);
        assert_eq!(parse_cue_time("1/0"), None);
        assert_eq!(parse_cue_time(""), None);
    }

    #[test]
    fn line_runs_cover_the_text_and_count_characters() {
        let text = "play 60\nsleep 0.5\n\n# héllo";
        let runs = line_runs(text);
        assert_eq!(runs.len(), 4);
        assert_eq!(runs[0].value, "play 60\n");
        assert_eq!(runs[2].value, "\n"); // empty line keeps its break
        assert_eq!(runs[3].value, "# héllo"); // final line: no break
        // character_lengths must sum to the value's byte length (é = 2 bytes).
        for run in &runs {
            let sum: usize = run.char_lengths.iter().map(|&b| b as usize).sum();
            assert_eq!(sum, run.value.len(), "run {:?}", run.value);
        }
        assert_eq!(runs[3].char_lengths, vec![1, 1, 1, 2, 1, 1, 1]);
    }

    #[test]
    fn word_starts_are_code_shaped() {
        //           0123456789
        assert_eq!(word_starts("play :bd_haus\n"), vec![0, 5, 6]);
        // leading whitespace is its own word; ident follows
        assert_eq!(word_starts("  play 60\n"), vec![0, 2, 7]);
    }

    #[test]
    fn byte_offsets_map_to_line_and_character_positions() {
        let text = "play 60\nsleep 1";
        assert_eq!(byte_to_text_position(text, 0), (0, 0));
        assert_eq!(byte_to_text_position(text, 4), (0, 4));
        // end of line 0 sits ON the line break (character index 7)
        assert_eq!(byte_to_text_position(text, 7), (0, 7));
        // one past the break = start of line 1
        assert_eq!(byte_to_text_position(text, 8), (1, 0));
        // end of document
        assert_eq!(byte_to_text_position(text, text.len()), (1, 7));
        // clamped past the end
        assert_eq!(byte_to_text_position(text, 999), (1, 7));
        // multibyte: caret after "é" in "hé" counts characters, not bytes
        assert_eq!(byte_to_text_position("hé\nx", 3), (0, 2));
    }
}

// ── Autocomplete: Sonic Pi vocabulary → the editor's completion menu ─────────

/// `CompletionProvider` over the repo-loaded [`Vocab`]: synths, fx, samples
/// (symbols) and lang functions, prefix-matched against the word at the caret.
struct SonicCompletions(Rc<Vocab>);

impl gpui_component::input::CompletionProvider for SonicCompletions {
    fn completions(
        &self,
        text: &gpui_component::input::Rope,
        offset: usize,
        _trigger: lsp_types::CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<gpui::Result<lsp_types::CompletionResponse>> {
        let text = text.to_string();
        let (word_start, prefix) = word_prefix_at(&text, offset);
        // Synth/fx opt names first (context-narrowed), then the vocabulary.
        let mut items: Vec<lsp_types::CompletionItem> = self
            .0
            .opt_completions(&text[..word_start], prefix, 20)
            .into_iter()
            .map(|(label, default)| lsp_types::CompletionItem {
                label: label.clone(),
                detail: Some(format!("opt · default {default}")),
                insert_text: Some(format!("{label} ")),
                kind: Some(lsp_types::CompletionItemKind::FIELD),
                ..Default::default()
            })
            .collect();
        items.extend(self.0.complete(prefix, 50).into_iter().map(|e| {
            lsp_types::CompletionItem {
                label: e.label.clone(),
                detail: Some(e.kind.label().to_string()),
                documentation: (!e.doc.is_empty())
                    .then(|| lsp_types::Documentation::String(e.doc.clone())),
                insert_text: Some(e.label.clone()),
                kind: Some(match e.kind {
                    VocabKind::Func => lsp_types::CompletionItemKind::FUNCTION,
                    VocabKind::Sample => lsp_types::CompletionItemKind::FILE,
                    _ => lsp_types::CompletionItemKind::VALUE,
                }),
                ..Default::default()
            }
        }));
        Task::ready(Ok(lsp_types::CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        new_text
            .chars()
            .last()
            .map(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
            .unwrap_or(false)
    }
}

/// `ApiClient` that queues raw engine events for the GUI thread, which drains
/// them each tick (log lines + error diagnostics). Real marshalling layer comes
/// with the real app.
struct LogSink(Arc<Mutex<Vec<ClientEvent>>>);

impl ApiClient for LogSink {
    fn on_event(&self, event: ClientEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// How the real runtime was booted: the classic Ruby daemon (4 processes) or
/// the Phase-4 Rust supervisor (3 — the app owns Spider + SuperSonic).
enum Runtime {
    DaemonRb(Daemon),
    Rust(Supervisor),
}

// ── Backend: real daemon session, or in-process loopback ────────────────────

enum Backend {
    /// The real thing: spawned `daemon.rb`, parsed handshake, live `Session`
    /// (senders + incoming OSC server + keep-alive).
    Real {
        session: Session,
        runtime: Runtime,
        /// SuperSonic's UDP port — names its shm segment (`/SuperSonic_<port>`).
        scsynth_port: u16,
    },
    /// Dev fallback: an in-process "spider" echoes engine messages so the whole
    /// OSC path runs without a built SuperSonic.
    Loopback {
        run_tx: UdpOscSender,
        token: i32,
        _gui_server: OscServer,
        _spider: OscServer,
    },
}

impl Backend {
    /// Boot chain: the Phase-4 Rust supervisor by DEFAULT (3 processes, app
    /// owns the children; all daemon features ported — 4560 cues, TOML
    /// audio opts, direct device switching), falling back to daemon.rb,
    /// then loopback. `SONIC_OXIDE_DAEMON=1` forces the daemon.rb path
    /// (the A/B oracle).
    fn connect(sink: Arc<Mutex<Vec<ClientEvent>>>) -> (Backend, String) {
        let force_daemon = std::env::var("SONIC_OXIDE_DAEMON").as_deref() == Ok("1");
        if !force_daemon {
            match Self::try_supervisor(sink.clone()) {
                Ok(b) => {
                    return (b, "=> Booted via Rust supervisor (3 processes, no daemon.rb).".into())
                }
                Err(why) => {
                    eprintln!("sonic-oxide: supervisor boot failed ({why}); trying daemon.rb");
                }
            }
        }
        match Self::try_real(sink.clone()) {
            Ok(b) => (b, "=> Connected to REAL Sonic Pi daemon.".into()),
            Err(why) => {
                let b = Self::loopback(sink);
                (b, format!("=> Real runtime unavailable ({why}); using loopback spider."))
            }
        }
    }

    fn try_supervisor(sink: Arc<Mutex<Vec<ClientEvent>>>) -> Result<Backend, String> {
        let root = app_root();
        let sup = Supervisor::boot(&root).map_err(|e| e.to_string())?;
        let session =
            Session::connect(&sup.ports, Arc::new(LogSink(sink))).map_err(|e| e.to_string())?;
        let scsynth_port = sup.ports.get(PortId::Scsynth);
        Ok(Backend::Real { session, runtime: Runtime::Rust(sup), scsynth_port })
    }

    fn try_real(sink: Arc<Mutex<Vec<ClientEvent>>>) -> Result<Backend, String> {
        let root = app_root();
        let paths = resolve(&root);
        let ruby = paths[&SonicPiPath::Ruby].clone();
        let script = paths[&SonicPiPath::BootDaemon].clone();
        if !script.exists() {
            return Err("daemon.rb not found".into());
        }

        // Boot with a timeout: if the daemon hangs we abandon the attempt (the
        // daemon self-terminates without keep-alive pings, so nothing leaks).
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Daemon::boot(&ruby, &script));
        });
        let daemon = match rx.recv_timeout(Duration::from_secs(4)) {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => return Err("handshake timeout".into()),
        };

        let session = Session::connect(&daemon.ports, Arc::new(LogSink(sink)))
            .map_err(|e| e.to_string())?;
        let scsynth_port = daemon.ports.get(PortId::Scsynth);
        Ok(Backend::Real { session, runtime: Runtime::DaemonRb(daemon), scsynth_port })
    }

    fn loopback(sink: Arc<Mutex<Vec<ClientEvent>>>) -> Backend {
        let token = 1;
        let client = LogSink(sink);
        let gui_server = OscServer::start(0, move |m| {
            if let Some(ev) = protocol::parse_incoming(&m) {
                client.on_event(ev);
            }
        })
        .expect("gui osc server");

        let reply = UdpOscSender::to_localhost(gui_server.port()).expect("reply sender");
        let spider = OscServer::start(0, move |m| match m.addr.as_str() {
            protocol::addr::SAVE_AND_RUN_BUFFER => {
                let (name, code) = match (m.args.get(1), m.args.get(2)) {
                    (Some(OscType::String(n)), Some(OscType::String(c))) => (n.clone(), c.clone()),
                    _ => (String::new(), String::new()),
                };
                let first = code.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                // Demo error path: a line containing `boom` triggers a syntax
                // error pointing at that line (1-based), like the real spider.
                if let Some(idx) = code.lines().position(|l| l.contains("boom")) {
                    let _ = reply.send(&osc(
                        protocol::addr::SYNTAX_ERROR,
                        vec![
                            OscType::Int(1),
                            OscType::String("undefined method `boom`".into()),
                            OscType::String(String::new()),
                            OscType::Int((idx + 1) as i32),
                        ],
                    ));
                    return;
                }
                let _ = reply.send(&osc(
                    protocol::addr::LOG_INFO,
                    vec![OscType::Int(0), OscType::String(format!("=> Run {name}"))],
                ));
                let _ = reply.send(&osc(
                    protocol::addr::LOG_MULTI_MESSAGE,
                    vec![
                        OscType::Int(1),
                        OscType::String(":main".into()),
                        OscType::String("0.0".into()),
                        OscType::Int(1),
                        OscType::Int(0),
                        OscType::String(format!("run → {first}")),
                    ],
                ));
                let _ = reply.send(&osc(
                    protocol::addr::RUNS_ALL_COMPLETED,
                    vec![OscType::String(name)],
                ));
            }
            protocol::addr::STOP_ALL_JOBS => {
                let _ = reply.send(&osc(
                    protocol::addr::LOG_INFO,
                    vec![OscType::Int(0), OscType::String("=> Stopping all runs".into())],
                ));
                let _ = reply.send(&osc(
                    protocol::addr::ACK,
                    vec![OscType::String("stop".into())],
                ));
            }
            _ => {}
        })
        .expect("spider osc server");

        let run_tx = UdpOscSender::to_localhost(spider.port()).expect("run sender");
        Backend::Loopback { run_tx, token, _gui_server: gui_server, _spider: spider }
    }

    fn run(&self, name: &str, code: &str) {
        match self {
            Backend::Real { session, .. } => {
                let _ = session.run(name, code);
            }
            Backend::Loopback { run_tx, token, .. } => {
                let _ = run_tx.send(&protocol::out::run_buffer(*token, name, code));
            }
        }
    }

    fn stop(&self) {
        match self {
            Backend::Real { session, .. } => {
                let _ = session.stop();
            }
            Backend::Loopback { run_tx, token, .. } => {
                let _ = run_tx.send(&protocol::out::stop_all_jobs(*token));
            }
        }
    }

    /// Polite shutdown of the real runtime: `/daemon/exit`, then give the
    /// daemon a moment to bring Spider/SuperSonic down before the kill
    /// backstop. Blocks (bounded) — called from the app-quit hook.
    /// Switch audio devices (real backend only). `None` leaves that side
    /// unchanged; `Some("__none__")` for input disables audio inputs. In
    /// daemon mode the request goes via the daemon (token-checked forward);
    /// in supervisor mode straight to the engine.
    fn switch_audio(&self, output: Option<&str>, input: Option<&str>) {
        if let Backend::Real { session, runtime, .. } = self {
            let (out, inp) = (output.unwrap_or(""), input.unwrap_or(""));
            let _ = match runtime {
                Runtime::DaemonRb(_) => session.switch_audio_device(out, 0.0, 0, inp),
                Runtime::Rust(_) => session.switch_audio_device_direct(out, 0.0, 0, inp),
            };
        }
    }

    /// Set the Link tempo (real backend only).
    fn set_bpm(&self, bpm: f32) {
        if let Backend::Real { session, .. } = self {
            let _ = session.send_to_supersonic(&protocol::out::clock_tempo_set(bpm));
        }
    }

    /// Start/stop SuperSonic's JUCE-side recorder (real backend only).
    fn record_start(&self, path: &str) {
        if let Backend::Real { session, .. } = self {
            let _ = session.send_to_supersonic(&protocol::out::record_start(path, "wav", 24));
        }
    }

    fn record_stop(&self) {
        if let Backend::Real { session, .. } = self {
            let _ = session.send_to_supersonic(&protocol::out::record_stop());
        }
    }

    /// Where this backend's engine publishes its scope shm, if known.
    fn scope_shm_name(&self) -> String {
        match self {
            Backend::Real { scsynth_port, .. } => format!("/SuperSonic_{scsynth_port}"),
            Backend::Loopback { .. } => SCOPE_SHM_NAME.to_string(),
        }
    }

    fn shutdown(&mut self) {
        if let Backend::Real { session, runtime, .. } = self {
            match runtime {
                Runtime::DaemonRb(daemon) => {
                    let _ = session.shutdown(); // polite /daemon/exit
                    if daemon.wait_timeout(Duration::from_secs(3)) {
                        eprintln!("sonic-oxide: daemon exited cleanly");
                    } else {
                        eprintln!("sonic-oxide: daemon didn't exit in 3s; killing");
                        daemon.kill();
                    }
                }
                Runtime::Rust(sup) => {
                    // Pre-shutdown liveness lets smoke tests assert the full
                    // runtime was actually up (a dead Spider would otherwise
                    // still produce a "clean" shutdown).
                    let (spider, engine) = sup.children_running();
                    eprintln!(
                        "sonic-oxide: runtime at quit — spider alive: {spider}, engine alive: {engine}"
                    );
                    if sup.shutdown_verified() {
                        eprintln!("sonic-oxide: supervisor children stopped (verified)");
                    } else {
                        eprintln!("sonic-oxide: WARNING — a supervisor child survived shutdown");
                    }
                }
            }
        }
    }
}

// ── Tier-2 a11y: full AccessKit Text support for the editor ──────────────────
//
// The AT-SPI `Text` interface (caret, selection, char/word/line navigation)
// only appears when a text-input node has `Role::TextRun` descendants carrying
// the text geometry (accesskit_consumer::supports_text_ranges). `set_value`
// alone is NOT enough — verified in plan/01-spike-results.md.
//
// Structure: `EditorA11y` (role MultilineTextInput) renders the real editor
// plus one zero-sized `TextRunA11y` child element per line. Each child pushes
// an AccessKit `TextRun` node (value = the line incl. its `\n`, per-character
// UTF-8 lengths, word starts). The field's `text_selection` must reference the
// run nodes' AccessKit NodeIds; those derive deterministically from each
// element's `GlobalElementId` (GPUI hashes it with std's DefaultHasher — we
// mirror that), but they're only knowable during the childrens' prepaint,
// which happens *after* the parent's `write_a11y_info`. So the children record
// their ids into a slab owned by the view, and the parent reads the *previous*
// frame's ids — stable across frames, so the selection is correct from the
// second frame on.

/// Mirror of `GlobalElementId::accesskit_node_id` (pub(crate) in gpui): the
/// same std DefaultHasher over the same Hash impl → the same NodeId.
fn accesskit_id_of(global_id: &GlobalElementId) -> accesskit::NodeId {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::default();
    global_id.hash(&mut hasher);
    accesskit::NodeId(hasher.finish())
}

/// One line of the buffer as AccessKit text-run data.
struct LineRun {
    /// Line text, including its trailing `\n` (except the final line).
    value: String,
    /// UTF-8 byte length of each character; sums to `value.len()`.
    char_lengths: Vec<u8>,
    /// Word start indices in characters (u8-capped, so truncated at 255).
    word_starts: Vec<u8>,
}

/// Split buffer text into per-line AccessKit runs.
fn line_runs(text: &str) -> Vec<LineRun> {
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let value = if i == last { line.to_string() } else { format!("{line}\n") };
            let char_lengths: Vec<u8> = value.chars().map(|c| c.len_utf8() as u8).collect();
            let word_starts = word_starts(&value);
            LineRun { value, char_lengths, word_starts }
        })
        .collect()
}

/// Word starts (in characters) for one run. Code-editor flavoured: a word is a
/// maximal run of identifier chars or of punctuation; whitespace attaches to
/// the word before it (per the AccessKit contract), except a line's leading
/// whitespace which is its own word.
fn word_starts(value: &str) -> Vec<u8> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Space,
        Ident,
        Punct,
    }
    let mut starts = Vec::new();
    let mut prev = None;
    for (i, c) in value.chars().enumerate() {
        if i > u8::MAX as usize {
            break; // property is u8-typed; caret positions are unaffected
        }
        let class = if c.is_whitespace() {
            Class::Space
        } else if c.is_alphanumeric() || c == '_' {
            Class::Ident
        } else {
            Class::Punct
        };
        let is_start = match prev {
            None => true,
            Some(Class::Space) => class != Class::Space,
            Some(p) => class != Class::Space && class != p,
        };
        // Leading whitespace of the line is its own word.
        let leading_ws_start = i == 0 && class == Class::Space;
        if is_start || leading_ws_start {
            starts.push(i as u8);
        }
        prev = Some(class);
    }
    starts
}

/// Map a byte offset in the buffer to (line index, character index within the
/// line's run). An offset pointing at a line's `\n` maps to that line break's
/// character (per AccessKit: caret at end of line sits ON the break).
fn byte_to_text_position(text: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(text.len());
    let mut line_start = 0usize;
    for (line_ix, line) in text.split('\n').enumerate() {
        // +1 for the '\n' (absent on the final line, hence >= below).
        let line_end = line_start + line.len();
        if byte <= line_end {
            let char_ix = text[line_start..byte].chars().count();
            return (line_ix, char_ix);
        }
        line_start = line_end + 1;
    }
    let last_line = text.split('\n').count() - 1;
    (last_line, text.rsplit('\n').next().unwrap_or("").chars().count())
}

/// Zero-sized child element publishing one `Role::TextRun` AccessKit node and
/// recording its NodeId into the view-owned slab for the parent's selection.
struct TextRunA11y {
    id: ElementId,
    run: LineRun,
    slot: usize,
    ids_out: Rc<RefCell<Vec<accesskit::NodeId>>>,
}

impl IntoElement for TextRunA11y {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextRunA11y {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        Some(accesskit::Role::TextRun)
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        node.set_value(self.run.value.clone());
        node.set_character_lengths(self.run.char_lengths.clone());
        node.set_word_starts(self.run.word_starts.clone());
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Absolutely positioned zero-size box: present in the trees, absent
        // from layout.
        let mut style = Style::default();
        style.position = gpui::Position::Absolute;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
        if let Some(gid) = id {
            let mut ids = self.ids_out.borrow_mut();
            if self.slot < ids.len() {
                ids[self.slot] = accesskit_id_of(gid);
            }
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _rl: &mut (),
        _pp: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

/// Wraps the editor: publishes a MultilineTextInput node with value, per-line
/// `TextRunA11y` children, and caret/selection mapped onto those runs.
struct EditorA11y {
    id: ElementId,
    value: SharedString,
    /// Selection as UTF-8 byte offsets (start ≤ end) + the caret (focus) end.
    selection: std::ops::Range<usize>,
    cursor: usize,
    run_ids: Rc<RefCell<Vec<accesskit::NodeId>>>,
    run_count: usize,
    runs: Vec<AnyElement>,
    inner: AnyElement,
}

impl EditorA11y {
    fn new(
        value: SharedString,
        selection: std::ops::Range<usize>,
        cursor: usize,
        run_ids: Rc<RefCell<Vec<accesskit::NodeId>>>,
        inner: AnyElement,
    ) -> Self {
        let runs: Vec<AnyElement> = line_runs(&value)
            .into_iter()
            .enumerate()
            .map(|(i, run)| {
                TextRunA11y {
                    id: ("editor-a11y-run", i).into(),
                    run,
                    slot: i,
                    ids_out: run_ids.clone(),
                }
                .into_any_element()
            })
            .collect();
        let run_count = runs.len();
        EditorA11y {
            id: "editor-a11y".into(),
            value,
            selection,
            cursor,
            run_ids,
            run_count,
            runs,
            inner,
        }
    }

    /// Byte offset → AccessKit TextPosition via last frame's run NodeIds.
    fn text_position(&self, byte: usize) -> Option<accesskit::TextPosition> {
        let ids = self.run_ids.borrow();
        let (line, character_index) = byte_to_text_position(&self.value, byte);
        ids.get(line).map(|node| accesskit::TextPosition { node: *node, character_index })
    }
}

impl IntoElement for EditorA11y {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorA11y {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        Some(accesskit::Role::MultilineTextInput)
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        node.set_label("Sonic Pi code editor");
        node.set_value(self.value.to_string());
        // Anchor = the fixed end (whichever selection end isn't the caret);
        // focus = the caret. Uses last frame's run ids (stable), so the very
        // first frame has no selection yet.
        let anchor_byte =
            if self.cursor == self.selection.end { self.selection.start } else { self.selection.end };
        if let (Some(anchor), Some(focus)) =
            (self.text_position(anchor_byte), self.text_position(self.cursor))
        {
            node.set_text_selection(accesskit::TextSelection { anchor, focus });
        }
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let inner = self.inner.request_layout(window, cx);
        let runs: Vec<LayoutId> =
            self.runs.iter_mut().map(|r| r.request_layout(window, cx)).collect();
        // A column that just stretches: the editor fills it exactly as it
        // filled this slot before; the runs are absolute zero-size boxes.
        let mut style = Style::default();
        style.display = Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.flex_grow = 1.0;
        style.size.width = relative(1.).into();
        style.min_size.height = px(0.).into();
        let children = std::iter::once(inner).chain(runs);
        (window.request_layout(style, children, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // Size the slab BEFORE the children record into it. write_a11y_info
        // already ran this frame using last frame's ids.
        {
            let mut ids = self.run_ids.borrow_mut();
            ids.resize(self.run_count, accesskit::NodeId(0));
            ids.truncate(self.run_count);
        }
        self.inner.prepaint(window, cx);
        for run in &mut self.runs {
            run.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _rl: &mut (),
        _pp: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(window, cx);
        for run in &mut self.runs {
            run.paint(window, cx);
        }
    }
}

// ── The app ──────────────────────────────────────────────────────────────────

/// Where the scope waveform comes from. The real engine publishes
/// triple-buffered scope slots (ScopeOut2); the loopback dev flow uses the
/// spike's fake audio ring (`shm_writer`). Attaching is lazy because the
/// engine creates its segment after the daemon handshake returns.
enum ScopeSource {
    RealSlot(ScopeSlotReader),
    FakeRing(ScopeReader),
    Detached,
}

struct SonicSpike {
    buffers: Vec<Entity<InputState>>,
    active: usize,
    cues: Entity<InputState>,
    scope: ScopeSource,
    /// Engine metrics (BPM, Link peers, …) from the same shm segment.
    metrics: Option<MetricsReader>,
    /// Live synth/group tree from the engine's shm mirror.
    node_tree: Option<NodeTreeReader>,
    nodes_open: bool,
    node_rows: Vec<NodeInfo>,
    node_version: u32,
    /// Show engine-internals rows in the Log (unmodelled OSC etc.).
    show_debug: bool,
    /// Collapsed panes (by pane id) — click a pane's title bar to fold it.
    collapsed: std::collections::HashSet<&'static str>,
    /// Latest real waveform window (kept between publishes so the scope
    /// doesn't blank when the engine is silent).
    scope_samples: Vec<f32>,
    /// Scope display mode: waveform or FFT spectrum.
    show_spectrum: bool,
    /// Rolling mono window feeding the FFT (last FRAME_SAMPLES samples).
    spectrum_rolling: Vec<f32>,
    spectrum_proc: SpectrumProcessor,
    spectrum_frame: Option<SpectrumFrame>,
    /// True once real data has flowed at least once; gates the demo sine.
    scope_live: bool,
    /// Tick countdown to the next (re)attach attempt while Detached.
    scope_retry: u32,
    phase: f32,
    running: bool,
    /// True between Run ▶ and the spider reporting all runs completed (or
    /// Stop ■). Drives the header state cue and the Stop button variant.
    playing: bool,
    /// Run-flash countdown (1.0 → 0.0); editor border highlights while > 0,
    /// and the Run button pulses while fresh.
    flash: f32,
    /// Stop-press countdown; pulses the Stop button on every press.
    stop_flash: f32,
    backend: Backend,
    incoming: Arc<Mutex<Vec<ClientEvent>>>,
    /// Workspace persistence: where buffers autosave, and the tick countdown.
    store_dir: PathBuf,
    autosave_in: u32,
    /// Latest device lists pushed by the engine; feed the settings pane.
    out_devices: Option<AudioDevicesInfo>,
    in_devices: Option<AudioInputDevicesInfo>,
    settings_open: bool,
    /// Previous tap-tempo press, for the interval → BPM conversion.
    last_tap: Option<std::time::Instant>,
    /// Path of the in-flight recording, if any.
    recording: Option<PathBuf>,
    /// Help pane: the loaded vocabulary doubles as the docs index.
    vocab: Rc<Vocab>,
    /// UI translations (i18n groundwork — pane titles wired first).
    i18n: Rc<i18n::I18n>,
    help_open: bool,
    help_query: Entity<InputState>,
    help_selected: Option<String>,
    log: Vec<LogLine>,
    /// Cue lines decoded from `/incoming/osc`, waiting for the next render
    /// (appending to the cues editor needs a `&mut Window`).
    pending_cues: Vec<String>,
    /// False until the first real cue replaces the placeholder text.
    cues_live: bool,
    /// First cue's absolute time — later cues display relative to it.
    first_cue_at: Option<f64>,
    /// AccessKit NodeIds of the editor's text-run children, recorded during
    /// prepaint and consumed (one frame later) for the a11y caret/selection.
    editor_run_ids: Rc<RefCell<Vec<accesskit::NodeId>>>,
}

impl SonicSpike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Completion vocabulary from the repo's own doc sources (one load,
        // shared by every buffer).
        let app_root = app_root();
        let vocab = Rc::new(Vocab::load(&app_root));
        let i18n = Rc::new(i18n::I18n::load(
            &app_root.join("../etc/i18n"),
            &i18n::I18n::detect_lang(),
        ));

        // Ten buffers: stored content wins, then seed, then empty.
        let store_dir = store::default_store_dir();
        let stored = store::load_buffers(&store_dir, BUFFER_COUNT);
        let buffers: Vec<Entity<InputState>> = (0..BUFFER_COUNT)
            .map(|i| {
                let initial: String = stored[i]
                    .clone()
                    .or_else(|| BUFFER_SEEDS.get(i).map(|s| s.to_string()))
                    .unwrap_or_default();
                let vocab = vocab.clone();
                cx.new(|cx| {
                    let mut state = InputState::new(window, cx)
                        .code_editor("ruby")
                        .line_number(true)
                        .soft_wrap(false)
                        .default_value(initial);
                    state.lsp.completion_provider =
                        Some(Rc::new(SonicCompletions(vocab.clone())));
                    state
                })
            })
            .collect();
        let cues = cx.new(|cx| {
            InputState::new(window, cx).multi_line(true).default_value(CUES_EXAMPLE)
        });
        let help_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search synths, samples, fns…"));

        let incoming: Arc<Mutex<Vec<ClientEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let (backend, status) = Backend::connect(incoming.clone());

        // Clean shutdown: save the workspace, then ask the daemon to exit
        // (and wait for it, bounded) before the app goes away. Runs
        // synchronously inside the quit hook — GPUI only polls the returned
        // future for SHUTDOWN_TIMEOUT (200ms), which is too short for the
        // daemon to tear down Spider/SuperSonic.
        cx.on_app_quit(|this: &mut Self, cx| {
            this.save_workspace(cx);
            this.backend.shutdown();
            async {}
        })
        .detach();

        // ~30fps tick: scope animation, run-flash decay, log drain.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                let alive = this
                    .update(cx, |this, cx| {
                        if this.running {
                            this.phase += 0.2;
                        }
                        this.flash = (this.flash - 0.06).max(0.0);
                        this.stop_flash = (this.stop_flash - 0.06).max(0.0);
                        this.autosave_in = this.autosave_in.saturating_sub(1);
                        if this.autosave_in == 0 {
                            this.autosave_in = AUTOSAVE_TICKS;
                            this.save_workspace(cx);
                        }
                        this.poll_scope();
                        let events: Vec<ClientEvent> =
                            std::mem::take(&mut *this.incoming.lock().unwrap());
                        for ev in &events {
                            this.log.extend(log_lines_for(ev));
                            // Live cues → the Cues pane (flushed on render),
                            // timestamped relative to the first cue.
                            if let ClientEvent::Cue(c) = ev {
                                let stamp = parse_cue_time(&c.time)
                                    .map(|t| {
                                        let base = *this.first_cue_at.get_or_insert(t);
                                        format!("+{:.3}s", t - base)
                                    })
                                    .unwrap_or_else(|| c.time.clone());
                                this.pending_cues
                                    .push(format!("{stamp}  {}  {}", c.address, c.args));
                            }
                            // Spider says every run has finished → not playing.
                            if let ClientEvent::Status(s) = ev {
                                if s.kind == StatusType::AllComplete {
                                    this.playing = false;
                                }
                            }
                            // Device pushes → the settings pane.
                            match ev {
                                ClientEvent::AudioDevices(d) => {
                                    this.out_devices = Some(d.clone());
                                }
                                ClientEvent::AudioInputDevices(d) => {
                                    this.in_devices = Some(d.clone());
                                }
                                _ => {}
                            }
                            // Error reports with a line number → editor diagnostic
                            // (red underline on that line in the active buffer).
                            if let ClientEvent::Report(m) = ev {
                                use sonicpi_core::MessageType as MT;
                                if matches!(m.kind, MT::RuntimeError | MT::SyntaxError)
                                    && m.line > 0
                                {
                                    let line = (m.line - 1).max(0) as u32;
                                    let diag = Diagnostic::new(
                                        Position::new(line, 0)..Position::new(line, 1000),
                                        m.text.clone(),
                                    )
                                    .with_severity(DiagnosticSeverity::Error);
                                    this.buffers[this.active].update(cx, |s, cx| {
                                        if let Some(set) = s.diagnostics_mut() {
                                            set.push(diag);
                                        }
                                        cx.notify();
                                    });
                                }
                            }
                        }
                        let overflow = this.log.len().saturating_sub(200);
                        if overflow > 0 {
                            this.log.drain(0..overflow);
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        Self {
            buffers,
            active: 0,
            cues,
            scope: ScopeSource::Detached,
            metrics: None,
            node_tree: None,
            nodes_open: false,
            node_rows: Vec::new(),
            node_version: 0,
            show_debug: false,
            collapsed: std::collections::HashSet::new(),
            scope_samples: Vec::new(),
            show_spectrum: false,
            spectrum_rolling: Vec::new(),
            spectrum_proc: SpectrumProcessor::new(),
            spectrum_frame: None,
            scope_live: false,
            scope_retry: 0,
            phase: 0.0,
            running: true,
            playing: false,
            flash: 0.0,
            stop_flash: 0.0,
            backend,
            incoming,
            store_dir,
            autosave_in: AUTOSAVE_TICKS,
            out_devices: None,
            in_devices: None,
            settings_open: false,
            last_tap: None,
            recording: None,
            vocab,
            i18n,
            help_open: false,
            help_query,
            help_selected: None,
            log: vec![LogLine::info(status)],
            pending_cues: Vec::new(),
            cues_live: false,
            first_cue_at: None,
            editor_run_ids: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Called from the 30fps tick: keep the scope fed. While detached, retry
    /// the attach about once a second; once attached, pull the newest window
    /// (keeping the previous one when nothing new was published).
    fn poll_scope(&mut self) {
        if !self.running {
            return; // scope paused
        }
        match &mut self.scope {
            ScopeSource::Detached => {
                if self.scope_retry > 0 {
                    self.scope_retry -= 1;
                    return;
                }
                self.scope_retry = 30;
                let name = self.backend.scope_shm_name();
                self.scope = match &self.backend {
                    Backend::Real { .. } => match ScopeSlotReader::open(&name, 0) {
                        Ok(r) => {
                            self.log.push(LogLine::info(format!(
                                "=> Scope attached to {name} (slot 0)"
                            )));
                            // Same segment, same moment: metrics + node tree.
                            self.metrics = MetricsReader::open(&name).ok();
                            self.node_tree = NodeTreeReader::open(&name).ok();
                            ScopeSource::RealSlot(r)
                        }
                        Err(_) => ScopeSource::Detached,
                    },
                    Backend::Loopback { .. } => match ScopeReader::open(&name) {
                        Ok(r) => ScopeSource::FakeRing(r),
                        Err(_) => ScopeSource::Detached,
                    },
                };
            }
            ScopeSource::RealSlot(r) => {
                if r.pull_latest_mono(&mut self.scope_samples) {
                    self.scope_live = true;
                    self.feed_spectrum();
                }
            }
            ScopeSource::FakeRing(r) => {
                let mut buf = Vec::new();
                if r.read_latest_mono(&mut buf, 1024) {
                    self.scope_samples = buf;
                    self.scope_live = true;
                    self.feed_spectrum();
                }
            }
        }
        // Refresh the node list only when the engine bumps its version.
        if self.nodes_open {
            if let Some(tree) = &self.node_tree {
                let v = tree.version();
                if v != self.node_version {
                    self.node_version = v;
                    self.node_rows = tree.read_nodes();
                }
            }
        }
        if self.show_spectrum && self.scope_live {
            let sample_rate = self
                .out_devices
                .as_ref()
                .map(|d| d.sample_rate as u32)
                .filter(|&r| r > 0)
                .unwrap_or(48_000);
            self.spectrum_frame =
                Some(self.spectrum_proc.process(&self.spectrum_rolling, sample_rate, 64));
        }
    }

    /// Append the newest scope window to the rolling FFT input (keep the
    /// most recent FRAME_SAMPLES).
    fn feed_spectrum(&mut self) {
        self.spectrum_rolling.extend_from_slice(&self.scope_samples);
        let overflow = self.spectrum_rolling.len().saturating_sub(spectrum::FRAME_SAMPLES);
        if overflow > 0 {
            self.spectrum_rolling.drain(0..overflow);
        }
    }

    /// Persist all buffers to the workspace store (autosave + quit path).
    fn save_workspace(&self, cx: &App) {
        let texts: Vec<String> =
            self.buffers.iter().map(|b| b.read(cx).value().to_string()).collect();
        store::save_buffers(&self.store_dir, &texts);
    }

    /// Run the active buffer (button and Alt+R share this).
    fn run_active(&mut self, cx: &mut Context<Self>) {
        let code = self.buffers[self.active].read(cx).value().to_string();
        // A fresh run clears the previous run's error underlines.
        self.buffers[self.active].update(cx, |s, cx| {
            if let Some(set) = s.diagnostics_mut() {
                set.clear();
            }
            cx.notify();
        });
        self.backend.run(&format!("buffer{}", self.active), &code);
        self.flash = 1.0;
        self.playing = true;
        // Local echo so the click lands in the Log instantly, before the
        // spider's own reply arrives.
        self.log.push(LogLine::info(format!("→ Run sent (buffer {})", self.active + 1)));
        cx.notify();
    }

    /// Stop all runs (button and Alt+S share this).
    fn stop_all(&mut self, cx: &mut Context<Self>) {
        self.backend.stop();
        self.playing = false;
        self.stop_flash = 1.0;
        self.log.push(LogLine::info("→ Stop sent"));
        cx.notify();
    }

    fn comment_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ed = self.buffers[self.active].clone();
        let (text, sel) = {
            let s = ed.read(cx);
            (s.value().to_string(), s.selected_range())
        };
        if let Some(new_text) = toggle_comment(&text, sel) {
            ed.update(cx, |s, cx| s.set_value(new_text, window, cx));
        }
        cx.notify();
    }

    fn align_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ed = self.buffers[self.active].clone();
        let text = ed.read(cx).value().to_string();
        let aligned = reindent(&text);
        if aligned != text {
            ed.update(cx, |s, cx| s.set_value(aligned, window, cx));
        }
        cx.notify();
    }

    fn select_buffer(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.buffers.len() {
            self.active = i;
            cx.notify();
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running;
        let playing = self.playing;
        // Every press pulses its button for ~250ms so repeat presses are
        // visible; steady state still encodes playing/idle.
        let run_pulsing = self.flash > 0.55;
        let stop_pulsing = self.stop_flash > 0.55;
        // Colour-only pulses: labels stay fixed so the header never reflows.
        let run = Button::new("run-code").label("Run ▶");
        let run = if run_pulsing { run.success() } else { run.primary() };
        let stop = Button::new("stop-code").label("Stop ■");
        let stop = if stop_pulsing {
            stop.warning()
        } else if playing {
            stop.danger()
        } else {
            stop.outline()
        };
        // Live engine metrics (BPM straight from the shm milli-BPM field).
        let metrics_text = self.metrics.as_ref().map(|m| {
            let bpm = m.link_bpm().unwrap_or(0.0);
            let peers = m.get(metrics_idx::LINK_PEERS).unwrap_or(0);
            format!("{bpm:.1} BPM · Link {peers}")
        });
        h_flex()
            .w_full()
            .p_2()
            .gap_2()
            .bg(cx.theme().title_bar)
            // Title doubles as the drag handle (no server-side decorations
            // on GNOME Wayland — we ARE the titlebar).
            .child(
                div()
                    .flex_1()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                    .child("Sonic Oxide"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(metrics_text.unwrap_or_default()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if playing { cx.theme().primary } else { cx.theme().muted_foreground })
                    .child(if playing { "● playing" } else { "○ idle" }),
            )
            .child(self.a11y_ctl(
                "a11y-run",
                "Run the current buffer",
                Cmd::Run,
                run.on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Run, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(self.a11y_ctl(
                "a11y-stop",
                "Stop all runs",
                Cmd::Stop,
                stop.on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Stop, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child({
                let rec = Button::new("record").label("⏺ Rec");
                let rec = if self.recording.is_some() { rec.danger() } else { rec.outline() };
                self.a11y_ctl(
                    "a11y-rec",
                    "Toggle recording",
                    Cmd::Rec,
                    rec.on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Rec, w, cx)))
                        .into_any_element(),
                    cx,
                )
            })
            .child(self.a11y_ctl(
                "a11y-comment",
                "Toggle comment on selection",
                Cmd::Comment,
                Button::new("toggle-comment")
                    .label("#")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Comment, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(self.a11y_ctl(
                "a11y-align",
                "Align buffer indentation",
                Cmd::Align,
                Button::new("align")
                    .label("⇥ Align")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Align, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(self.a11y_ctl(
                "a11y-scope-pause",
                "Pause or resume the scope",
                Cmd::ScopePause,
                Button::new("scope-toggle")
                    .label(if running { "Scope ⏸" } else { "Scope ▶" })
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::ScopePause, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(self.a11y_ctl(
                "a11y-scope-mode",
                "Switch scope between waveform and spectrum",
                Cmd::ScopeMode,
                Button::new("scope-mode")
                    .label(if self.show_spectrum { "〰 Wave" } else { "▁▃▅ Spec" })
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::ScopeMode, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(
                Button::new("theme-toggle")
                    .label(if cx.theme().is_dark() { "☀" } else { "☾" })
                    .on_click(cx.listener(|_, _, window, cx| {
                        let next = if cx.theme().is_dark() {
                            gpui_component::ThemeMode::Light
                        } else {
                            gpui_component::ThemeMode::Dark
                        };
                        gpui_component::Theme::change(next, Some(window), cx);
                        cx.notify();
                    })),
            )
            .child(self.a11y_ctl(
                "a11y-settings",
                "Toggle settings pane",
                Cmd::Settings,
                Button::new("settings-toggle")
                    .label("⚙")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Settings, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(self.a11y_ctl(
                "a11y-help",
                "Toggle help pane",
                Cmd::Help,
                Button::new("help-toggle")
                    .label("?")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Help, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(self.a11y_ctl(
                "a11y-nodes",
                "Toggle node tree pane",
                Cmd::Nodes,
                Button::new("nodes-toggle")
                    .label("♪")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Nodes, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child({
                let btn = Button::new("debug-toggle").label("Dbg");
                let btn = if self.show_debug { btn.primary() } else { btn };
                self.a11y_ctl(
                    "a11y-debug",
                    "Toggle debug log rows",
                    Cmd::Debug,
                    btn.on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Debug, w, cx)))
                        .into_any_element(),
                    cx,
                )
            })
            // Window controls: GNOME Wayland gives us no server-side
            // decorations, so minimise/maximise/close live here. Close goes
            // through remove_window → on_window_closed → the clean-shutdown
            // quit path.
            .child(div().w_4())
            .child(
                Button::new("win-min")
                    .label("–")
                    .on_click(|_, window, _| window.minimize_window()),
            )
            .child(
                Button::new("win-max")
                    .label("□")
                    .on_click(|_, window, _| window.zoom_window()),
            )
            .child(
                Button::new("win-close")
                    .label("✕")
                    .on_click(|_, window, _| window.remove_window()),
            )
    }

    /// Help pane: search the vocabulary, click an entry, read its docs
    /// (summary + synth/fx opts) — the cheatsheets and lang doc blocks are
    /// the same sources the full Qt help renders.
    fn help(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.help_query.read(cx).value().to_string();
        let hits: Vec<(String, &'static str)> = self
            .vocab
            .search(&query, 10)
            .into_iter()
            .map(|e| (e.label.clone(), e.kind.label()))
            .collect();

        let mut pane = v_flex()
            .gap_1()
            .p_2()
            .text_sm()
            .child(Input::new(&self.help_query).small());

        if !hits.is_empty() {
            let mut row = h_flex().gap_1().flex_wrap();
            for (i, (label, kind)) in hits.iter().enumerate() {
                let selected = self.help_selected.as_deref() == Some(label.as_str());
                let btn = Button::new(("help-hit", i))
                    .xsmall()
                    .label(format!("{label} ({kind})"));
                let btn = if selected { btn.primary() } else { btn.outline() };
                let label = label.clone();
                row = row.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                    this.help_selected = Some(label.clone());
                    cx.notify();
                })));
            }
            pane = pane.child(row);
        } else if !query.trim().is_empty() {
            pane = pane.child(div().text_xs().child("No matches."));
        }

        if let Some(entry) = self.help_selected.as_deref().and_then(|l| self.vocab.get(l)) {
            let mut body = v_flex().gap_1().child(
                div().text_xs().text_color(cx.theme().muted_foreground).child(format!(
                    "{} — {}",
                    entry.label,
                    entry.kind.label()
                )),
            );
            if !entry.doc.is_empty() {
                body = body.child(div().text_sm().child(entry.doc.clone()));
            }
            if !entry.long_doc.is_empty() {
                // Full lang doc body (bounded — some run to pages).
                let mut text: String = entry.long_doc.chars().take(1200).collect();
                if text.len() < entry.long_doc.len() {
                    text.push('…');
                }
                body = body.child(
                    div().text_xs().text_color(cx.theme().muted_foreground).child(text),
                );
            }
            if !entry.opts.is_empty() {
                body = body.child(
                    div()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(entry.opts.join("   ")),
                );
            }
            pane = pane.child(body);
        }

        pane.into_any_element()
    }

    /// Nudge the Link tempo by `delta` BPM (metrics give the current value).
    fn nudge_bpm(&mut self, delta: f32) {
        let current =
            self.metrics.as_ref().and_then(|m| m.link_bpm()).unwrap_or(60.0) as f32;
        let next = (current + delta).clamp(20.0, 400.0);
        self.backend.set_bpm(next);
        self.log.push(LogLine::info(format!("→ BPM → {next:.1}")));
    }

    /// Tap tempo: two taps ≤ 3s apart set the BPM from the interval.
    fn tap_tempo(&mut self) {
        let now = std::time::Instant::now();
        if let Some(prev) = self.last_tap.replace(now) {
            let secs = now.duration_since(prev).as_secs_f32();
            if secs > 0.0 && secs < 3.0 {
                let bpm = (60.0 / secs).clamp(20.0, 400.0);
                self.backend.set_bpm(bpm);
                self.log.push(LogLine::info(format!("→ Tap tempo → {bpm:.1} BPM")));
            }
        }
    }

    /// Settings pane: audio output/input pickers driven by the engine's own
    /// device pushes (`/supersonic/devices` + `/supersonic/input-devices`),
    /// Link tempo controls, and a live engine-metrics strip. Scrolls — device
    /// lists can be long.
    fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut pane = v_flex().id("settings-scroll").gap_2().p_2().text_sm().size_full();

        // Link tempo row.
        let bpm = self.metrics.as_ref().and_then(|m| m.link_bpm());
        pane = pane.child(
            h_flex()
                .gap_1()
                .items_center()
                .child(div().text_xs().child(match bpm {
                    Some(b) => format!("Tempo {b:.1} BPM"),
                    None => "Tempo (waiting for engine)".to_string(),
                }))
                .child(Button::new("bpm-down").xsmall().outline().label("−5").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.nudge_bpm(-5.0);
                        cx.notify();
                    }),
                ))
                .child(Button::new("bpm-up").xsmall().outline().label("+5").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.nudge_bpm(5.0);
                        cx.notify();
                    }),
                ))
                .child(Button::new("bpm-tap").xsmall().outline().label("Tap").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.tap_tempo();
                        cx.notify();
                    }),
                )),
        );

        // Engine metrics strip (self-describing shm metrics array).
        if let Some(m) = &self.metrics {
            let line = format!(
                "engine: {} callbacks · {} msgs · queue {} · Link peers {}",
                m.get(metrics_idx::PROCESS_COUNT).unwrap_or(0),
                m.get(metrics_idx::MESSAGES_PROCESSED).unwrap_or(0),
                m.get(metrics_idx::SCHEDULER_QUEUE_DEPTH).unwrap_or(0),
                m.get(metrics_idx::LINK_PEERS).unwrap_or(0),
            );
            pane = pane.child(div().text_xs().text_color(cx.theme().muted_foreground).child(line));
        }

        match &self.out_devices {
            Some(d) => {
                pane = pane.child(div().text_xs().child(format!(
                    "Output ({} @ {}Hz)",
                    d.mode, d.sample_rate
                )));
                let current = d.current_device.clone();
                for (i, name) in d.devices.iter().enumerate() {
                    let btn = Button::new(("out-dev", i)).xsmall().label(name.clone());
                    let btn = if *name == current { btn.primary() } else { btn.outline() };
                    let name = name.clone();
                    pane = pane.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                        this.backend.switch_audio(Some(&name), None);
                        this.log.push(LogLine::info(format!("→ Output device → {name}")));
                        cx.notify();
                    })));
                }
            }
            None => {
                pane = pane
                    .child(div().text_xs().child("Output devices: waiting for the engine…"));
            }
        }

        match &self.in_devices {
            Some(d) => {
                pane = pane.child(div().text_xs().child("Input"));
                let current = d.current_device.clone();
                for (i, name) in d.devices.iter().enumerate() {
                    let btn = Button::new(("in-dev", i)).xsmall().label(name.clone());
                    let btn = if *name == current { btn.primary() } else { btn.outline() };
                    let name = name.clone();
                    pane = pane.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                        this.backend.switch_audio(None, Some(&name));
                        this.log.push(LogLine::info(format!("→ Input device → {name}")));
                        cx.notify();
                    })));
                }
                pane = pane.child(
                    Button::new("in-dev-none").xsmall().outline().label("Disable input").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.backend.switch_audio(None, Some("__none__"));
                            this.log.push(LogLine::info("→ Audio input disabled"));
                            cx.notify();
                        }),
                    ),
                );
            }
            None => {
                pane =
                    pane.child(div().text_xs().child("Input devices: waiting for the engine…"));
            }
        }

        // Scroll within the pane instead of overflowing it.
        pane.overflow_y_scroll().into_any_element()
    }

    /// Start/stop recording (Rec button + its a11y action share this).
    fn toggle_record(&mut self, cx: &mut Context<Self>) {
        match self.recording.take() {
            Some(path) => {
                self.backend.record_stop();
                self.log.push(LogLine::info(format!("→ Recording saved: {}", path.display())));
            }
            None => {
                let dir = self.store_dir.join("recordings");
                let _ = std::fs::create_dir_all(&dir);
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let path = dir.join(format!("rec-{stamp}.wav"));
                self.backend.record_start(&path.to_string_lossy());
                self.log.push(LogLine::info(format!("→ Recording to {}", path.display())));
                self.recording = Some(path);
            }
        }
        cx.notify();
    }

    /// One dispatch point for header commands: mouse clicks and screen-reader
    /// Click actions both land here.
    fn dispatch(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Cmd::Run => self.run_active(cx),
            Cmd::Stop => self.stop_all(cx),
            Cmd::Rec => self.toggle_record(cx),
            Cmd::Comment => self.comment_active(window, cx),
            Cmd::Align => self.align_active(window, cx),
            Cmd::ScopePause => {
                self.running = !self.running;
                cx.notify();
            }
            Cmd::ScopeMode => {
                self.show_spectrum = !self.show_spectrum;
                cx.notify();
            }
            Cmd::Settings => {
                self.settings_open = !self.settings_open;
                cx.notify();
            }
            Cmd::Help => {
                self.help_open = !self.help_open;
                cx.notify();
            }
            Cmd::Nodes => {
                self.nodes_open = !self.nodes_open;
                self.node_version = 0;
                cx.notify();
            }
            Cmd::Debug => {
                self.show_debug = !self.show_debug;
                cx.notify();
            }
        }
    }

    /// Wrap a control so screen readers see (and can activate) it: a Button
    /// node with a label and a Click action routed through `dispatch` —
    /// gpui-component's own widgets emit no AccessKit nodes yet.
    fn a11y_ctl(
        &self,
        id: &'static str,
        label: &str,
        cmd: Cmd,
        inner: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Screen-reader label goes through the translation table too.
        let label = SharedString::from(self.i18n.tr(label).to_string());
        let weak = cx.entity().downgrade();
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .on_a11y_action(accesskit::Action::Click, move |_, window, app| {
                if let Some(ent) = weak.upgrade() {
                    ent.update(app, |this, cx| this.dispatch(cmd, window, cx));
                }
            })
            .child(inner)
            .into_any_element()
    }

    /// Live node tree: groups and synths from the engine's shm mirror,
    /// indented by parent depth.
    fn nodes(&self, cx: &Context<Self>) -> AnyElement {
        if self.node_tree.is_none() {
            return div().p_2().text_xs().child("Node tree: waiting for the engine…").into_any_element();
        }
        let parents: std::collections::HashMap<i32, i32> =
            self.node_rows.iter().map(|n| (n.id, n.parent_id)).collect();
        let depth = |mut id: i32| {
            let mut d = 0;
            while d < 8 {
                match parents.get(&id) {
                    Some(&p) if p >= 0 => {
                        id = p;
                        d += 1;
                    }
                    _ => break,
                }
            }
            d
        };
        let muted = cx.theme().muted_foreground;
        let fg = cx.theme().foreground;
        v_flex()
            .p_2()
            .text_xs()
            .font_family(cx.theme().mono_font_family.clone())
            .child(div().text_color(muted).child(format!("{} node(s)", self.node_rows.len())))
            .children(self.node_rows.iter().map(|n| {
                let pad = "  ".repeat(depth(n.id));
                let (icon, color) = if n.is_group { ("▸", muted) } else { ("♪", fg) };
                div()
                    .text_color(color)
                    .child(format!("{pad}{icon} {} [{}]", n.name, n.id))
            }))
            .into_any_element()
    }

    fn tab_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active;
        h_flex().gap_1().p_1().children((0..self.buffers.len()).map(move |i| {
            let btn = Button::new(("buffer-tab", i)).label(format!("{}", i + 1)).xsmall();
            let btn = if i == active { btn.primary() } else { btn };
            btn.on_click(cx.listener(move |this, _, _, cx| this.select_buffer(i, cx)))
        }))
    }

    /// Titled pane with accessibility wiring (id/role/label). `flash` drives
    /// the run-flash border highlight (editor only). Clicking the title bar
    /// collapses/expands the pane (dock-lite; the full DockArea comes later).
    fn pane(
        &self,
        id: &'static str,
        title: &str,
        role: Role,
        a11y_label: impl Into<SharedString>,
        flash: f32,
        child: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let border = if flash > 0.0 { cx.theme().primary } else { cx.theme().border };
        let collapsed = self.collapsed.contains(id);
        let marker = if collapsed { "▸" } else { "▾" };
        let root = v_flex()
            .id(id)
            .role(role)
            .aria_label(a11y_label)
            .min_h(px(24.))
            // Long content must shrink with the pane, not widen the layout
            // past the window (which pushed the titlebar controls off the
            // right edge on small windows).
            .min_w(px(0.))
            .border_1()
            .border_color(border)
            .child(
                div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .bg(cx.theme().secondary)
                    .text_xs()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if !this.collapsed.remove(id) {
                                this.collapsed.insert(id);
                            }
                            cx.notify();
                        }),
                    )
                    .child(format!("{marker} {title}")),
            );
        if collapsed {
            // Header only: no flex growth, body dropped entirely.
            root
        } else {
            root
                .flex_1()
                // Clip the body: an overflowing pane must never paint over
                // its siblings (a long device list once bled across three).
                .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(child))
        }
    }

    /// Spectrum analyser bars + peak-hold markers from the latest FFT frame.
    fn spectrum_el(&self, cx: &Context<Self>) -> AnyElement {
        let color = cx.theme().primary;
        let peak_color = cx.theme().danger;
        let frame = self.spectrum_frame.clone().unwrap_or_default();
        div()
            .w_full()
            .h(px(150.))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, _| {
                        if frame.bars.is_empty() {
                            return;
                        }
                        let bw = f32::from(bounds.size.width);
                        let bh = f32::from(bounds.size.height);
                        let x0 = f32::from(bounds.origin.x);
                        let y0 = f32::from(bounds.origin.y);
                        let bar_w = (bw / frame.bars.len() as f32).max(1.0);
                        for (i, (&bar, &peak)) in
                            frame.bars.iter().zip(frame.peaks.iter()).enumerate()
                        {
                            let x = x0 + i as f32 * bar_w;
                            let h = bar * bh;
                            window.paint_quad(fill(
                                Bounds {
                                    origin: point(px(x), px(y0 + bh - h)),
                                    size: size(px((bar_w * 0.8).max(1.0)), px(h.max(1.0))),
                                },
                                color,
                            ));
                            // Peak-hold marker: a thin line above the bar.
                            let py = y0 + bh - peak * bh;
                            window.paint_quad(fill(
                                Bounds {
                                    origin: point(px(x), px(py)),
                                    size: size(px((bar_w * 0.8).max(1.0)), px(2.0)),
                                },
                                peak_color,
                            ));
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    /// (element, is_live): latest engine window once data has flowed, demo
    /// sine otherwise. The window itself is pulled in `poll_scope`.
    fn scope(&self, cx: &Context<Self>) -> (AnyElement, bool) {
        let live = self.scope_live && !self.scope_samples.is_empty();
        if self.show_spectrum && live {
            return (self.spectrum_el(cx), true);
        }
        let color = cx.theme().primary;
        let mut samples: Vec<f32> = if live { self.scope_samples.clone() } else { Vec::new() };

        if !live {
            let n = 128usize;
            let phase = self.phase;
            samples = (0..n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    ((t * std::f32::consts::TAU * 3.0) + phase).sin() * (1.0 - t).max(0.0)
                })
                .collect();
        }

        let el = div()
            .w_full()
            .h(px(150.))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, _| {
                        if samples.is_empty() {
                            return;
                        }
                        let bw = f32::from(bounds.size.width);
                        let bh = f32::from(bounds.size.height);
                        let mid = f32::from(bounds.origin.y) + bh / 2.0;
                        let bar_w = (bw / samples.len() as f32).max(1.0);
                        for (i, s) in samples.iter().enumerate() {
                            let h = (s.abs() * bh / 2.0).max(1.0);
                            let x = f32::from(bounds.origin.x) + i as f32 * bar_w;
                            let y = if *s >= 0.0 { mid - h } else { mid };
                            let bar = Bounds {
                                origin: point(px(x), px(y)),
                                size: size(px((bar_w * 0.8).max(1.0)), px(h)),
                            };
                            window.paint_quad(fill(bar, color));
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element();
        (el, live)
    }
}

impl Render for SonicSpike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Flush queued cues into the (selectable) cues editor; the first real
        // cue replaces the placeholder example.
        if !self.pending_cues.is_empty() {
            let fresh: Vec<String> = self.pending_cues.drain(..).collect();
            let cues = self.cues.clone();
            let was_live = std::mem::replace(&mut self.cues_live, true);
            cues.update(cx, |s, cx| {
                let mut text = if was_live { s.value().to_string() } else { String::new() };
                for line in fresh {
                    text.push_str(&line);
                    text.push('\n');
                }
                // Cap the scrollback (keep the newest ~200 lines).
                let overflow = text.lines().count().saturating_sub(200);
                if overflow > 0 {
                    text = text.lines().skip(overflow).collect::<Vec<_>>().join("\n") + "\n";
                }
                s.set_value(text, window, cx);
            });
        }

        let editor = self.buffers[self.active].clone();
        let (scope_el, scope_live) = self.scope(cx);

        let code = editor.read(cx).value();
        let caret = editor.read(cx).cursor();
        let editor_label: SharedString = format!(
            "Sonic Pi code editor, buffer {}, {} characters, cursor at offset {}",
            self.active + 1,
            code.len(),
            caret
        )
        .into();
        let cues_label: SharedString = format!("Cues log. {}", self.cues.read(cx).value()).into();
        let log_label: SharedString = format!(
            "Run log. {}",
            self.log.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(". ")
        )
        .into();
        let show_debug = self.show_debug;
        let recent: Vec<LogLine> = self
            .log
            .iter()
            .rev()
            .filter(|l| show_debug || !l.debug)
            .take(16)
            .cloned()
            .collect();
        let log_fg = cx.theme().foreground;
        let log_err = cx.theme().danger;

        let selection = editor.read(cx).selected_range();
        let editor_body = v_flex()
            .size_full()
            .child(self.tab_row(cx))
            .child(EditorA11y::new(
                code,
                selection,
                caret,
                self.editor_run_ids.clone(),
                Input::new(&editor)
                    .size_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(cx.theme().mono_font_size)
                    .into_any_element(),
            ));

        let settings_el = self.settings_open.then(|| self.settings(cx));
        let help_el = self.help_open.then(|| self.help(cx));
        let nodes_el = self.nodes_open.then(|| self.nodes(cx));
        let mut right = v_flex().size_full().gap_2().p_2();
        if let Some(el) = nodes_el {
            right = right.child(self.pane(
                "nodes",
                self.i18n.tr("Nodes"),
                Role::Group,
                "Live synth node tree",
                0.0,
                el,
                cx,
            ));
        }
        if let Some(el) = help_el {
            right = right.child(self.pane(
                "help",
                self.i18n.tr("Help"),
                Role::Group,
                "Documentation search",
                0.0,
                el,
                cx,
            ));
        }
        if let Some(el) = settings_el {
            right = right.child(self.pane(
                "settings",
                self.i18n.tr("Settings"),
                Role::Group,
                "Audio settings",
                0.0,
                el,
                cx,
            ));
        }
        // The permanent panes are a vertical resizable group — drag the
        // dividers to re-balance scope/cues/log (dock-lite, part two).
        let core_panes = v_resizable("right-panes")
            .child(resizable_panel().child(self.pane(
                "scope",
                if scope_live { "Scope · live (shm)" } else { "Scope · demo" },
                Role::Image,
                "Audio scope waveform",
                0.0,
                scope_el,
                cx,
            )))
            .child(resizable_panel().child(self.pane(
                "cues",
                self.i18n.tr("Cues"),
                Role::Group,
                cues_label,
                0.0,
                Input::new(&self.cues).h_full().into_any_element(),
                cx,
            )))
            .child(resizable_panel().child(self.pane(
                "log",
                self.i18n.tr("Log"),
                Role::Group,
                log_label,
                0.0,
                v_flex()
                    .size_full()
                    .p_1()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .children(recent.into_iter().map(move |l| {
                        let color = if l.error {
                            log_err
                        } else {
                            l.run.map(run_color).unwrap_or(log_fg)
                        };
                        let row = div().text_color(color);
                        let row = if l.indent { row.pl_4() } else { row };
                        row.child(l.text)
                    }))
                    .into_any_element(),
                cx,
            )));
        let right = right.child(div().flex_1().min_h(px(0.)).child(core_panes));

        v_flex()
            .id("sonic-spike")
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &RunBuffer, _, cx| this.run_active(cx)))
            .on_action(cx.listener(|this, _: &StopAll, _, cx| this.stop_all(cx)))
            .on_action(
                cx.listener(|this, _: &CommentToggle, window, cx| this.comment_active(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &AlignBuffer, window, cx| this.align_active(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextBuffer, _, cx| {
                let next = (this.active + 1) % this.buffers.len();
                this.select_buffer(next, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevBuffer, _, cx| {
                let prev = (this.active + this.buffers.len() - 1) % this.buffers.len();
                this.select_buffer(prev, cx);
            }))
            .child(self.header(cx))
            .child(
                h_resizable("main")
                    .child(resizable_panel().child(self.pane(
                        "editor",
                        self.i18n.tr("Editor"),
                        Role::Group,
                        editor_label,
                        self.flash,
                        editor_body,
                        cx,
                    )))
                    .child(resizable_panel().size(px(460.)).child(right)),
            )
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(Assets);
    app.run(|cx| {
        gpui_component::init(cx);
        // Sonic Pi is dark by default; ☀/☾ in the header toggles.
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        cx.activate(true);

        // Live-coder shortcuts (Qt parity: Alt+R run, Alt+S stop, …). Global
        // context so they fire even while the editor holds focus.
        cx.bind_keys([
            KeyBinding::new("alt-r", RunBuffer, None),
            KeyBinding::new("alt-s", StopAll, None),
            KeyBinding::new("alt-/", CommentToggle, None),
            KeyBinding::new("alt-m", AlignBuffer, None),
            KeyBinding::new("alt-]", NextBuffer, None),
            KeyBinding::new("alt-[", PrevBuffer, None),
        ]);

        // Closing the last window quits the app (which fires the on_app_quit
        // shutdown hook); without this the process — and the daemon — lingers.
        cx.on_window_closed(|cx, _id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // Test/CI hook: SONIC_SPIKE_AUTOQUIT=<secs> quits through the same
        // graceful path as a window close, so clean shutdown is verifiable
        // headlessly (a Wayland WM close can't be scripted).
        if let Some(secs) =
            std::env::var("SONIC_SPIKE_AUTOQUIT").ok().and_then(|v| v.parse::<u64>().ok())
        {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(secs)).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }

        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Sonic Oxide".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| SonicSpike::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open window");
    });
}
