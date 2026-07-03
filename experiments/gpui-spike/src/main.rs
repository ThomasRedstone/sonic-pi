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
mod tutorial;
mod vocab;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use vocab::{word_prefix_at, Vocab, VocabKind};

// Live-coding keyboard shortcuts (bound in `main`, handled on the root view).
actions!(sonic_spike, [RunBuffer, StopAll, CommentToggle, AlignBuffer, NextBuffer, PrevBuffer, ZoomIn, ZoomOut, ZoomReset, OpenFile, SaveFile, SaveFileAs]);

/// Header commands — one identity shared by mouse clicks, keyboard actions
/// and screen-reader Click actions.
#[derive(Clone, Copy)]
enum Cmd {
    Run,
    Stop,
    Rec,
    Open,
    Save,
    Comment,
    Align,
    ScopePause,
    ScopeMode,
    Settings,
    Help,
    Tutorial,
    Nodes,
    Debug,
}

use gpui::*;
use gpui_component::{
    ActiveTheme, Root, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockItem, Panel, PanelEvent},
    h_flex,
    highlighter::{Diagnostic, DiagnosticSeverity},
    input::{Input, InputState, Position},
    resizable::{h_resizable, resizable_panel},
    slider::{Slider, SliderEvent, SliderState},
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
    protocol, ApiClient, AudioDevicesInfo, AudioDriversInfo, AudioInputDevicesInfo, ClientEvent,
    Session, StatusType,
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

    fn error(text: impl Into<String>) -> LogLine {
        LogLine { run: None, error: true, indent: false, debug: false, text: text.into() }
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
        ClientEvent::AudioDrivers(d) => vec![LogLine::info(format!(
            "[audio] {} driver(s), current: {}",
            d.drivers.len(),
            d.current
        ))],
        ClientEvent::DriverSwitched { ok: true, detail } => {
            vec![LogLine::info(format!("[audio] driver switched: {detail}"))]
        }
        ClientEvent::DriverSwitched { ok: false, detail } => {
            vec![LogLine::error(format!("driver switch failed: {detail}"))]
        }
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

/// The `#__nosave__` preamble Qt's `runCode` prepends to every run — same
/// lines, same order. Spider subtracts leading `#__nosave__` lines from error
/// line numbers (runtime.rb) and strips them on buffer save, so this is
/// invisible to the user. `midi_channel` is `"*"` (all) or `"1"`..`"16"`.
/// Pure → unit-tested.
fn run_preamble(safe: bool, external_synths: bool, timing: bool, midi_channel: &str) -> String {
    const SUFFIX: &str = " #__nosave__ set by Sonic Oxide user preferences.\n";
    let mut p = format!("use_midi_defaults channel: \"{midi_channel}\"{SUFFIX}");
    if timing {
        p.push_str(&format!("use_timing_guarantees true{SUFFIX}"));
    }
    if external_synths {
        p.push_str(&format!("use_external_synths true{SUFFIX}"));
    }
    if safe {
        p.push_str(&format!("use_arg_checks true{SUFFIX}"));
    }
    p
}

/// Recent-files list codec for prefs.conf (single line, tab-separated —
/// prefs values only trim end whitespace). Newest first, deduped, capped.
const RECENT_CAP: usize = 8;

fn decode_recent(value: &str) -> Vec<PathBuf> {
    value.split('\t').filter(|s| !s.is_empty()).map(PathBuf::from).collect()
}

fn encode_recent(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\t")
}

fn push_recent(mut recent: Vec<PathBuf>, path: &Path) -> Vec<PathBuf> {
    recent.retain(|p| p != path);
    recent.insert(0, path.to_path_buf());
    recent.truncate(RECENT_CAP);
    recent
}

/// MIDI default-channel stepping: `* → 1 → … → 16 → *` (and back). Matches
/// Qt's combo values ("*" first, then 1..16). Pure → unit-tested.
fn next_midi_channel(ch: &str) -> String {
    match ch.parse::<u8>() {
        Ok(n) if n >= 16 => "*".into(),
        Ok(n) => (n + 1).to_string(),
        Err(_) => "1".into(),
    }
}

fn prev_midi_channel(ch: &str) -> String {
    match ch.parse::<u8>() {
        Ok(n) if n <= 1 => "*".into(),
        Ok(n) => (n - 1).to_string(),
        Err(_) => "16".into(),
    }
}

/// Save-dialog filename suggestion: keep the buffer's known name, else
/// `buffer-N.rb`. Pure → unit-tested.
fn suggested_save_name(known: Option<&Path>, buffer_ix: usize) -> String {
    known
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| format!("buffer-{}.rb", buffer_ix + 1))
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
    fn preamble_mirrors_qt_run_code() {
        use super::run_preamble;
        // Defaults: safe mode on, channel "*" — two lines, midi first.
        let p = run_preamble(true, false, false, "*");
        let lines: Vec<&str> = p.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("use_midi_defaults channel: \"*\""));
        assert!(lines[1].starts_with("use_arg_checks true"));
        assert!(lines.iter().all(|l| l.contains("#__nosave__")));

        // Everything on: 4 lines, Qt's prepend order.
        let p = run_preamble(true, true, true, "10");
        let lines: Vec<&str> = p.lines().collect();
        assert!(lines[0].contains("channel: \"10\""));
        assert!(lines[1].starts_with("use_timing_guarantees true"));
        assert!(lines[2].starts_with("use_external_synths true"));
        assert!(lines[3].starts_with("use_arg_checks true"));

        // All off: the midi line always rides along (Qt does the same).
        assert_eq!(run_preamble(false, false, false, "*").lines().count(), 1);
    }

    #[test]
    fn recent_files_dedupe_cap_and_round_trip() {
        use super::{decode_recent, encode_recent, push_recent, RECENT_CAP};
        use std::path::{Path, PathBuf};
        let mut recent = Vec::new();
        for i in 0..12 {
            recent = push_recent(recent, Path::new(&format!("/tmp/file{i}.rb")));
        }
        assert_eq!(recent.len(), RECENT_CAP);
        assert_eq!(recent[0], PathBuf::from("/tmp/file11.rb")); // newest first
        // Re-opening an existing file moves it to the front without duping.
        recent = push_recent(recent, Path::new("/tmp/file5.rb"));
        assert_eq!(recent[0], PathBuf::from("/tmp/file5.rb"));
        assert_eq!(recent.iter().filter(|p| **p == PathBuf::from("/tmp/file5.rb")).count(), 1);
        // Prefs round-trip.
        assert_eq!(decode_recent(&encode_recent(&recent)), recent);
        assert!(decode_recent("").is_empty());
    }

    #[test]
    fn midi_channel_steps_cycle_star_and_1_to_16() {
        use super::{next_midi_channel, prev_midi_channel};
        assert_eq!(next_midi_channel("*"), "1");
        assert_eq!(next_midi_channel("1"), "2");
        assert_eq!(next_midi_channel("16"), "*");
        assert_eq!(prev_midi_channel("*"), "16");
        assert_eq!(prev_midi_channel("1"), "*");
        assert_eq!(prev_midi_channel("16"), "15");
    }

    #[test]
    fn save_name_prefers_the_known_filename() {
        use super::suggested_save_name;
        use std::path::Path;
        assert_eq!(suggested_save_name(None, 0), "buffer-1.rb");
        assert_eq!(suggested_save_name(None, 9), "buffer-10.rb");
        assert_eq!(
            suggested_save_name(Some(Path::new("/tmp/riff.rb")), 3),
            "riff.rb"
        );
    }

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

    /// Master volume (0.0..=2.0), sent to Spider (real backend only).
    fn set_volume(&self, amp: f32) {
        if let Backend::Real { session, .. } = self {
            let _ = session.set_mixer_amp(amp, false);
        }
    }

    /// Spider-side settings toggles (real backend only; silent no-ops on
    /// loopback, like the other setters).
    fn set_midi(&self, on: bool) {
        if let Backend::Real { session, .. } = self {
            let _ = session.set_midi_enabled(on, true);
        }
    }

    fn set_cue_server(&self, on: bool) {
        if let Backend::Real { session, .. } = self {
            let _ = session.set_cue_server_enabled(on);
        }
    }

    fn set_cue_external(&self, on: bool) {
        if let Backend::Real { session, .. } = self {
            let _ = session.set_cue_server_external(on);
        }
    }

    fn set_invert_stereo(&self, on: bool) {
        if let Backend::Real { session, .. } = self {
            let _ = session.set_mixer_invert_stereo(on);
        }
    }

    fn set_force_mono(&self, on: bool) {
        if let Backend::Real { session, .. } = self {
            let _ = session.set_mixer_force_mono(on);
        }
    }

    /// Switch the audio driver: daemon-brokered in daemon.rb mode, direct to
    /// the engine in supervisor mode (mirrors the device-switch split).
    fn switch_driver(&self, driver: &str) {
        if let Backend::Real { session, runtime, .. } = self {
            let _ = match runtime {
                Runtime::DaemonRb(_) => session.switch_audio_driver(driver),
                Runtime::Rust(_) => session.switch_audio_driver_direct(driver),
            };
        }
    }

    fn request_drivers(&self) {
        if let Backend::Real { session, .. } = self {
            let _ = session.request_audio_drivers();
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

/// Scope display modes (Qt ScopeWindow parity: mono, stereo, mirror,
/// Lissajous, spectrum). The header button cycles through them.
#[derive(Clone, Copy, PartialEq)]
enum ScopeMode {
    Mono,
    Stereo,
    Mirror,
    Lissajous,
    Spectrum,
}

impl ScopeMode {
    fn next(self) -> ScopeMode {
        match self {
            ScopeMode::Mono => ScopeMode::Stereo,
            ScopeMode::Stereo => ScopeMode::Mirror,
            ScopeMode::Mirror => ScopeMode::Lissajous,
            ScopeMode::Lissajous => ScopeMode::Spectrum,
            ScopeMode::Spectrum => ScopeMode::Mono,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ScopeMode::Mono => "Mono",
            ScopeMode::Stereo => "Stereo",
            ScopeMode::Mirror => "Mirror",
            ScopeMode::Lissajous => "Liss",
            ScopeMode::Spectrum => "Spec",
        }
    }
}

struct SonicSpike {
    buffers: Vec<Entity<InputState>>,
    /// Where each buffer was opened from / last saved to (Ctrl+S reuses it;
    /// buffers never opened or saved prompt on save). Persisted per buffer,
    /// so tabs keep their file names across restarts.
    file_paths: Vec<Option<PathBuf>>,
    /// Recently opened/saved files, newest first (persisted, cap 8).
    recent: Vec<PathBuf>,
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
    /// doesn't blank when the engine is silent). Left channel / mono.
    scope_samples: Vec<f32>,
    /// Right channel of the same window (mono sources mirror the left).
    scope_right: Vec<f32>,
    /// Scope display mode (mono/stereo/mirror/Lissajous/spectrum).
    scope_mode: ScopeMode,
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
    /// The right-hand dock: scope/cues/log as draggable/rearrangeable panels.
    dock: Option<Entity<DockArea>>,
    /// Workspace persistence: where buffers autosave, and the tick countdown.
    store_dir: PathBuf,
    autosave_in: u32,
    /// Latest device lists pushed by the engine; feed the settings pane.
    out_devices: Option<AudioDevicesInfo>,
    in_devices: Option<AudioInputDevicesInfo>,
    /// Audio driver enumeration (`/supersonic/drivers/list.reply`).
    drivers: Option<AudioDriversInfo>,
    settings_open: bool,
    /// Settings prefs (persisted in prefs.conf). Spider-side ones re-apply on
    /// SpiderReady; the run-semantics ones ride each run's preamble.
    safe_mode: bool,
    timing_guarantees: bool,
    external_synths: bool,
    /// `"*"` (all channels) or `"1"`..`"16"`.
    midi_channel: String,
    midi_enabled: bool,
    cue_server: bool,
    cue_external: bool,
    invert_stereo: bool,
    force_mono: bool,
    /// Current UI language + the discovered choices.
    lang: String,
    langs: Vec<String>,
    /// Previous tap-tempo press, for the interval → BPM conversion.
    last_tap: Option<std::time::Instant>,
    /// Master volume slider (0..2, default 1 — mirrors the Qt preamp).
    volume: Entity<SliderState>,
    /// Editor font size (Ctrl+=/-/0), persisted in prefs.conf.
    font_size: f32,
    /// Path of the in-flight recording, if any.
    recording: Option<PathBuf>,
    /// Help pane: the loaded vocabulary doubles as the docs index.
    vocab: Rc<Vocab>,
    /// UI translations (i18n groundwork — pane titles wired first).
    i18n: Rc<i18n::I18n>,
    help_open: bool,
    help_query: Entity<InputState>,
    help_selected: Option<String>,
    /// Tutorial + examples browser (Tier 2.1). Content loads on first open.
    tutorial_open: bool,
    chapters: Option<Vec<tutorial::Chapter>>,
    examples: Option<Vec<tutorial::Example>>,
    /// Selected chapter index + its markdown source.
    chapter_ix: Option<usize>,
    chapter_body: SharedString,
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

        // Ten buffers: stored content wins, then seed, then empty.
        let store_dir = store::default_store_dir();
        let prefs = store::load_prefs(&store_dir);
        let font_size: f32 =
            prefs.get("font_size").and_then(|v| v.parse().ok()).unwrap_or(14.0);
        // Language: pref wins over the environment.
        let lang = prefs.get("lang").cloned().unwrap_or_else(i18n::I18n::detect_lang);
        let i18n_dir = app_root.join("../etc/i18n");
        let i18n = Rc::new(i18n::I18n::load(&i18n_dir, &lang));
        let langs = i18n::I18n::available_langs(&i18n_dir);
        let pref_bool =
            |key: &str, default: bool| prefs.get(key).map(|v| v == "true").unwrap_or(default);
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
        let volume = cx.new(|_| {
            SliderState::new().min(0.0).max(2.0).step(0.05).default_value(1.0)
        });
        cx.subscribe(&volume, |this, _, ev: &SliderEvent, _| {
            if let SliderEvent::Change(v) = ev {
                if let gpui_component::slider::SliderValue::Single(amp) = v {
                    this.backend.set_volume(*amp);
                }
            }
        })
        .detach();

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
                            // Device/driver pushes → the settings pane.
                            match ev {
                                ClientEvent::AudioDevices(d) => {
                                    this.out_devices = Some(d.clone());
                                }
                                ClientEvent::AudioInputDevices(d) => {
                                    this.in_devices = Some(d.clone());
                                }
                                ClientEvent::AudioDrivers(d) => {
                                    this.drivers = Some(d.clone());
                                }
                                // Success changes the current driver — refresh
                                // the enumeration.
                                ClientEvent::DriverSwitched { ok: true, .. } => {
                                    this.backend.request_drivers();
                                }
                                // Spider is up: re-apply the persisted
                                // spider-side settings (MIDI, cue server,
                                // mixer shape).
                                ClientEvent::SpiderReady => {
                                    this.apply_runtime_prefs();
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

        // Dock: scope/cues/log become rearrangeable panels (drag a tab onto
        // another panel to stack/tabs them, or between dividers to re-split).
        let app_entity = cx.entity();
        let dock = cx.new(|cx| DockArea::new("oxide-dock", Some(1), window, cx));
        let weak_dock = dock.downgrade();
        let scope_p = cx.new(|c| ScopePanel::new(app_entity.clone(), c));
        let cues_p = cx.new(|c| CuesPanel::new(app_entity.clone(), c));
        let log_p = cx.new(|c| LogPanel::new(app_entity.clone(), c));
        let center = DockItem::v_split(
            vec![
                DockItem::tab(scope_p, &weak_dock, window, cx),
                DockItem::tab(cues_p, &weak_dock, window, cx),
                DockItem::tab(log_p, &weak_dock, window, cx),
            ],
            &weak_dock,
            window,
            cx,
        );
        dock.update(cx, |d, cx| d.set_center(center, window, cx));

        let file_paths: Vec<Option<PathBuf>> = (0..BUFFER_COUNT)
            .map(|i| prefs.get(&format!("buffer_path_{i}")).map(PathBuf::from))
            .collect();
        let recent = prefs.get("recent_files").map(|v| decode_recent(v)).unwrap_or_default();

        Self {
            buffers,
            file_paths,
            recent,
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
            scope_right: Vec::new(),
            scope_mode: ScopeMode::Mono,
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
            dock: Some(dock),
            store_dir,
            autosave_in: AUTOSAVE_TICKS,
            out_devices: None,
            in_devices: None,
            drivers: None,
            settings_open: false,
            // Qt defaults: safe mode ON, the rest off/all/enabled.
            safe_mode: pref_bool("safe_mode", true),
            timing_guarantees: pref_bool("timing_guarantees", false),
            external_synths: pref_bool("external_synths", false),
            midi_channel: prefs.get("midi_channel").cloned().unwrap_or_else(|| "*".into()),
            midi_enabled: pref_bool("midi_enabled", true),
            cue_server: pref_bool("cue_server", true),
            cue_external: pref_bool("cue_external", false),
            invert_stereo: pref_bool("invert_stereo", false),
            force_mono: pref_bool("force_mono", false),
            lang,
            langs,
            last_tap: None,
            recording: None,
            volume,
            font_size,
            vocab,
            i18n,
            help_open: false,
            help_query,
            help_selected: None,
            tutorial_open: false,
            chapters: None,
            examples: None,
            chapter_ix: None,
            chapter_body: SharedString::default(),
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
                if r.pull_latest_stereo(&mut self.scope_samples, &mut self.scope_right) {
                    self.scope_live = true;
                    self.feed_spectrum();
                }
            }
            ScopeSource::FakeRing(r) => {
                let mut buf = Vec::new();
                if r.read_latest_mono(&mut buf, 1024) {
                    self.scope_samples = buf;
                    // The fake ring is mono; stereo modes mirror it.
                    self.scope_right = self.scope_samples.clone();
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
        if self.scope_mode == ScopeMode::Spectrum && self.scope_live {
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

    /// Tier 1.3: open a `.rb`/text file into the active buffer via the
    /// platform's native file dialog. The chosen path sticks to the buffer,
    /// so a later Ctrl+S saves straight back to it.
    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let text = std::fs::read_to_string(&path);
            let _ = this.update_in(cx, |this, window, cx| {
                let i = this.active;
                match text {
                    Ok(text) => {
                        this.buffers[i].update(cx, |s, cx| s.set_value(text, window, cx));
                        this.remember_file(i, &path);
                        this.log.push(LogLine::info(format!(
                            "→ Opened {} into buffer {}",
                            path.display(),
                            i + 1
                        )));
                    }
                    Err(e) => this
                        .log
                        .push(LogLine::error(format!("Open {} failed: {e}", path.display()))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Save the active buffer. With a known path (and `!save_as`) it writes
    /// straight there; otherwise the native save dialog picks the target.
    fn save_file(&mut self, save_as: bool, window: &mut Window, cx: &mut Context<Self>) {
        let i = self.active;
        if !save_as {
            if let Some(path) = self.file_paths[i].clone() {
                self.write_buffer_to(i, &path, cx);
                cx.notify();
                return;
            }
        }
        let dir = self.file_paths[i]
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = suggested_save_name(self.file_paths[i].as_deref(), i);
        let rx = cx.prompt_for_new_path(&dir, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let _ = this.update_in(cx, |this, _, cx| {
                this.remember_file(i, &path);
                this.write_buffer_to(i, &path, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn write_buffer_to(&mut self, i: usize, path: &Path, cx: &Context<Self>) {
        let text = self.buffers[i].read(cx).value().to_string();
        match std::fs::write(path, &text) {
            Ok(()) => self.log.push(LogLine::info(format!(
                "→ Saved buffer {} → {}",
                i + 1,
                path.display()
            ))),
            Err(e) => {
                self.log.push(LogLine::error(format!("Save {} failed: {e}", path.display())))
            }
        }
    }

    /// Adjust the editor font size (clamped 8..40) and persist it.
    fn zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.font_size = if delta == 0.0 { 14.0 } else { (self.font_size + delta).clamp(8.0, 40.0) };
        let mut prefs = store::load_prefs(&self.store_dir);
        prefs.insert("font_size".to_string(), format!("{}", self.font_size));
        store::save_prefs(&self.store_dir, &prefs);
        cx.notify();
    }

    /// Persist all buffers to the workspace store (autosave + quit path).
    fn save_workspace(&self, cx: &App) {
        let texts: Vec<String> =
            self.buffers.iter().map(|b| b.read(cx).value().to_string()).collect();
        store::save_buffers(&self.store_dir, &texts);
    }

    /// Associate `path` with buffer `i`: sticky save target, file-stem tab
    /// name, and a spot at the head of the recent list — all persisted.
    fn remember_file(&mut self, i: usize, path: &Path) {
        self.file_paths[i] = Some(path.to_path_buf());
        self.recent = push_recent(std::mem::take(&mut self.recent), path);
        let mut prefs = store::load_prefs(&self.store_dir);
        prefs.insert(format!("buffer_path_{i}"), path.to_string_lossy().into_owned());
        prefs.insert("recent_files".into(), encode_recent(&self.recent));
        store::save_prefs(&self.store_dir, &prefs);
    }

    /// Persist one settings pref (read-modify-write keeps the others).
    fn set_pref(&self, key: &str, value: impl ToString) {
        let mut prefs = store::load_prefs(&self.store_dir);
        prefs.insert(key.to_string(), value.to_string());
        store::save_prefs(&self.store_dir, &prefs);
    }

    /// Push the persisted spider-side settings at the runtime (idempotent —
    /// sent on SpiderReady and whenever toggled).
    fn apply_runtime_prefs(&self) {
        self.backend.set_midi(self.midi_enabled);
        self.backend.set_cue_server(self.cue_server);
        self.backend.set_cue_external(self.cue_external);
        self.backend.set_invert_stereo(self.invert_stereo);
        self.backend.set_force_mono(self.force_mono);
    }

    /// Run the active buffer (button and Alt+R share this).
    fn run_active(&mut self, cx: &mut Context<Self>) {
        let user_code = self.buffers[self.active].read(cx).value().to_string();
        // Qt parity: run-semantics prefs ride a #__nosave__ preamble (error
        // lines stay buffer-relative — Spider subtracts these lines).
        let code = format!(
            "{}{}",
            run_preamble(
                self.safe_mode,
                self.external_synths,
                self.timing_guarantees,
                &self.midi_channel
            ),
            user_code
        );
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
        let run = Button::new("run-code").label(format!("{} ▶", self.i18n.tr("Run")));
        let run = if run_pulsing { run.success() } else { run.primary() };
        let stop = Button::new("stop-code").label(format!("{} ■", self.i18n.tr("Stop")));
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
                    .id("volume")
                    .role(Role::Slider)
                    .aria_label("Master volume")
                    .w(px(110.))
                    .child(Slider::new(&self.volume)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if playing { cx.theme().primary } else { cx.theme().muted_foreground })
                    .child(if playing {
                        format!("● {}", self.i18n.tr("playing"))
                    } else {
                        format!("○ {}", self.i18n.tr("idle"))
                    }),
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
                let rec = Button::new("record").label(format!("⏺ {}", self.i18n.tr("Rec")));
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
                "a11y-open",
                "Open a file into the current buffer",
                Cmd::Open,
                Button::new("open-file")
                    .label("📂")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Open, w, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(self.a11y_ctl(
                "a11y-save",
                "Save the current buffer to a file",
                Cmd::Save,
                Button::new("save-file")
                    .label("💾")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Save, w, cx)))
                    .into_any_element(),
                cx,
            ))
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
                    .label(format!("⇥ {}", self.i18n.tr("Align")))
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
                "Cycle scope display mode",
                Cmd::ScopeMode,
                Button::new("scope-mode")
                    .label(format!("〰 {}", self.scope_mode.label()))
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
                "a11y-tutorial",
                "Toggle tutorial pane",
                Cmd::Tutorial,
                Button::new("tutorial-toggle")
                    .label("📖")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Cmd::Tutorial, w, cx)))
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
            pane = pane.child(div().text_xs().child(self.i18n.tr("No matches.").to_string()));
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

    /// Tutorial + examples browser: chapter nav on the left, rendered
    /// markdown on the right; examples load straight into the active buffer.
    fn tutorial(&self, cx: &mut Context<Self>) -> AnyElement {
        let chapters = self.chapters.as_deref().unwrap_or(&[]);
        let examples = self.examples.as_deref().unwrap_or(&[]);
        let muted = cx.theme().muted_foreground;

        // Left column: every chapter in curriculum order, then the examples.
        let mut nav = v_flex().id("tutorial-nav").w(px(190.)).overflow_y_scroll().gap_0p5().p_1();
        for (i, ch) in chapters.iter().enumerate() {
            let btn = Button::new(("tut-ch", i)).xsmall().label(ch.title.clone());
            let btn = if self.chapter_ix == Some(i) { btn.primary() } else { btn.ghost() };
            nav = nav.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                if let Some(ch) = this.chapters.as_ref().and_then(|c| c.get(i)) {
                    this.chapter_body =
                        std::fs::read_to_string(&ch.path).unwrap_or_default().into();
                    this.chapter_ix = Some(i);
                }
                cx.notify();
            })));
        }
        nav = nav.child(
            div().px_1().pt_2().text_xs().text_color(muted).child(
                self.i18n.tr("Examples").to_string(),
            ),
        );
        let mut last_cat = "";
        for (i, ex) in examples.iter().enumerate() {
            if ex.category != last_cat {
                last_cat = &ex.category;
                nav = nav.child(
                    div().px_1().text_xs().text_color(muted).child(ex.category.clone()),
                );
            }
            nav = nav.child(
                Button::new(("tut-ex", i)).xsmall().ghost().label(format!("♪ {}", ex.name)).on_click(
                    cx.listener(move |this, _, window, cx| {
                        let Some(ex) = this.examples.as_ref().and_then(|e| e.get(i)) else {
                            return;
                        };
                        match std::fs::read_to_string(&ex.path) {
                            Ok(code) => {
                                let b = this.active;
                                this.buffers[b]
                                    .update(cx, |s, cx| s.set_value(code, window, cx));
                                this.log.push(LogLine::info(format!(
                                    "→ Example {}/{} loaded into buffer {}",
                                    ex.category,
                                    ex.name,
                                    b + 1
                                )));
                            }
                            Err(e) => this.log.push(LogLine::error(format!(
                                "Example {} failed: {e}",
                                ex.path.display()
                            ))),
                        }
                        cx.notify();
                    }),
                ),
            );
        }

        // Right: the chapter markdown (gpui-component TextView), or a hint.
        let body: AnyElement = if self.chapter_ix.is_some() {
            div()
                .id("tutorial-body")
                .flex_1()
                .min_w(px(0.))
                .overflow_y_scroll()
                .p_2()
                .text_sm()
                .child(gpui_component::text::markdown(self.chapter_body.clone()))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .p_2()
                .text_sm()
                .text_color(muted)
                .child(self.i18n.tr("Pick a chapter — or load an example into the buffer.").to_string())
                .into_any_element()
        };

        h_flex().size_full().items_start().child(nav.h_full()).child(body).into_any_element()
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
                    Some(b) => format!("{} {b:.1} BPM", self.i18n.tr("Tempo")),
                    None => format!("{} ({})", self.i18n.tr("Tempo"), self.i18n.tr("waiting for engine")),
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
                    .child(div().text_xs().child(format!("{}…", self.i18n.tr("Output devices: waiting for the engine"))));
            }
        }

        match &self.in_devices {
            Some(d) => {
                pane = pane.child(div().text_xs().child(self.i18n.tr("Input").to_string()));
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
                    Button::new("in-dev-none").xsmall().outline().label(self.i18n.tr("Disable input").to_string()).on_click(
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
                    pane.child(div().text_xs().child(format!("{}…", self.i18n.tr("Input devices: waiting for the engine"))));
            }
        }

        // Audio driver picker (supervisor mode gets the enumeration reply;
        // clicking hot-swaps via JUCE, which then re-reports devices).
        match &self.drivers {
            Some(d) => {
                pane = pane.child(div().text_xs().child(format!(
                    "{} ({})",
                    self.i18n.tr("Audio driver"),
                    d.current
                )));
                let current = d.current.clone();
                let mut row = h_flex().gap_1().flex_wrap();
                for (i, name) in d.drivers.iter().enumerate() {
                    let btn = Button::new(("drv", i)).xsmall().label(name.clone());
                    let btn = if *name == current { btn.primary() } else { btn.outline() };
                    let name = name.clone();
                    row = row.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                        this.backend.switch_driver(&name);
                        this.log.push(LogLine::info(format!("→ Audio driver → {name}")));
                        cx.notify();
                    })));
                }
                pane = pane.child(row);
            }
            None => {
                pane = pane.child(div().text_xs().child(format!(
                    "{}…",
                    self.i18n.tr("Audio driver: waiting for the engine")
                )));
            }
        }

        // Mixer shape (live spider toggles).
        pane = pane.child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .child(self.setting_toggle("invert-stereo", "Invert stereo", self.invert_stereo, cx, |this, v| {
                    this.invert_stereo = v;
                    this.set_pref("invert_stereo", v);
                    this.backend.set_invert_stereo(v);
                }))
                .child(self.setting_toggle("force-mono", "Force mono", self.force_mono, cx, |this, v| {
                    this.force_mono = v;
                    this.set_pref("force_mono", v);
                    this.backend.set_force_mono(v);
                })),
        );

        // MIDI: enable + default channel ("*" = all, 1..16 — rides the run
        // preamble as use_midi_defaults, exactly like Qt).
        let midi_ch = self.midi_channel.clone();
        pane = pane.child(
            h_flex()
                .gap_1()
                .items_center()
                .child(self.setting_toggle("midi-enable", "MIDI", self.midi_enabled, cx, |this, v| {
                    this.midi_enabled = v;
                    this.set_pref("midi_enabled", v);
                    this.backend.set_midi(v);
                }))
                .child(div().text_xs().child(format!("{} {midi_ch}", self.i18n.tr("Default channel"))))
                .child(Button::new("midi-ch-down").xsmall().outline().label("−").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.midi_channel = prev_midi_channel(&this.midi_channel);
                        this.set_pref("midi_channel", &this.midi_channel);
                        cx.notify();
                    }),
                ))
                .child(Button::new("midi-ch-up").xsmall().outline().label("+").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.midi_channel = next_midi_channel(&this.midi_channel);
                        this.set_pref("midi_channel", &this.midi_channel);
                        cx.notify();
                    }),
                )),
        );

        // Network OSC (spider cue server).
        pane = pane.child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .child(self.setting_toggle("cue-server", "Allow incoming OSC", self.cue_server, cx, |this, v| {
                    this.cue_server = v;
                    this.set_pref("cue_server", v);
                    this.backend.set_cue_server(v);
                }))
                .child(self.setting_toggle("cue-external", "Allow remote OSC", self.cue_external, cx, |this, v| {
                    this.cue_external = v;
                    this.set_pref("cue_external", v);
                    this.backend.set_cue_external(v);
                })),
        );

        // Run semantics (preamble-applied on the next Run).
        pane = pane.child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .child(self.setting_toggle("safe-mode", "Safe mode", self.safe_mode, cx, |this, v| {
                    this.safe_mode = v;
                    this.set_pref("safe_mode", v);
                }))
                .child(self.setting_toggle("timing-guarantees", "Timing guarantees", self.timing_guarantees, cx, |this, v| {
                    this.timing_guarantees = v;
                    this.set_pref("timing_guarantees", v);
                }))
                .child(self.setting_toggle("external-synths", "External synths", self.external_synths, cx, |this, v| {
                    this.external_synths = v;
                    this.set_pref("external_synths", v);
                })),
        );

        // Theme catalogue (built-ins + anything in etc/themes).
        let themes: Vec<(SharedString, bool)> = {
            let active = cx.theme().theme_name().clone();
            gpui_component::ThemeRegistry::global(cx)
                .sorted_themes()
                .iter()
                .map(|t| (t.name.clone(), t.name == active))
                .collect()
        };
        pane = pane.child(div().text_xs().child(self.i18n.tr("Theme").to_string()));
        let mut row = h_flex().gap_1().flex_wrap();
        for (i, (name, current)) in themes.into_iter().enumerate() {
            let btn = Button::new(("theme", i)).xsmall().label(name.to_string());
            let btn = if current { btn.primary() } else { btn.outline() };
            row = row.child(btn.on_click(cx.listener(move |this, _, window, cx| {
                let cfg = gpui_component::ThemeRegistry::global(cx)
                    .themes()
                    .get(&name)
                    .cloned();
                if let Some(cfg) = cfg {
                    gpui_component::Theme::global_mut(cx).apply_config(&cfg);
                    gpui_component::Theme::change(cfg.mode, Some(window), cx);
                    this.set_pref("theme", &name);
                }
                cx.notify();
            })));
        }
        pane = pane.child(row);

        // Language picker (live reload; completes the i18n mechanism).
        pane = pane.child(div().text_xs().child(self.i18n.tr("Language").to_string()));
        let mut row = h_flex().gap_1().flex_wrap();
        for (i, l) in self.langs.clone().into_iter().enumerate() {
            let btn = Button::new(("lang", i)).xsmall().label(l.clone());
            let btn = if l == self.lang { btn.primary() } else { btn.outline() };
            row = row.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                this.lang = l.clone();
                this.set_pref("lang", &l);
                this.i18n =
                    Rc::new(i18n::I18n::load(&app_root().join("../etc/i18n"), &l));
                cx.notify();
            })));
        }
        pane = pane.child(row);

        // Scroll within the pane instead of overflowing it.
        pane.overflow_y_scroll().into_any_element()
    }

    /// One ☑/☐ settings toggle button; `apply` mutates state + sends/persists.
    fn setting_toggle(
        &self,
        id: &'static str,
        label: &str,
        on: bool,
        cx: &mut Context<Self>,
        apply: impl Fn(&mut Self, bool) + 'static,
    ) -> AnyElement {
        let text = format!("{} {}", if on { "☑" } else { "☐" }, self.i18n.tr(label));
        let btn = Button::new(id).xsmall().label(text);
        let btn = if on { btn.primary() } else { btn.outline() };
        btn.on_click(cx.listener(move |this, _, _, cx| {
            apply(this, !on);
            cx.notify();
        }))
        .into_any_element()
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
            Cmd::Open => self.open_file(window, cx),
            Cmd::Save => self.save_file(false, window, cx),
            Cmd::Comment => self.comment_active(window, cx),
            Cmd::Align => self.align_active(window, cx),
            Cmd::ScopePause => {
                self.running = !self.running;
                cx.notify();
            }
            Cmd::ScopeMode => {
                self.scope_mode = self.scope_mode.next();
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
            Cmd::Tutorial => {
                self.tutorial_open = !self.tutorial_open;
                if self.tutorial_open && self.chapters.is_none() {
                    let root = app_root();
                    self.chapters = Some(tutorial::load_chapters(&root.join("../etc/doc/tutorial")));
                    self.examples = Some(tutorial::load_examples(&root.join("../etc/examples")));
                }
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
        // Tabs carry their file name once a buffer is tied to one.
        let labels: Vec<String> = (0..self.buffers.len())
            .map(|i| match &self.file_paths[i] {
                Some(p) => p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("{}", i + 1)),
                None => format!("{}", i + 1),
            })
            .collect();
        let mut row = h_flex().gap_1().p_1().flex_wrap();
        for (i, label) in labels.into_iter().enumerate() {
            let btn = Button::new(("buffer-tab", i)).label(label).xsmall();
            let btn = if i == active { btn.primary() } else { btn };
            row = row
                .child(btn.on_click(cx.listener(move |this, _, _, cx| this.select_buffer(i, cx))));
        }
        // Recent files: one click re-opens into the ACTIVE buffer.
        if !self.recent.is_empty() {
            row = row.child(div().text_xs().text_color(cx.theme().muted_foreground).child(
                format!("{}:", self.i18n.tr("Recent")),
            ));
            for (i, path) in self.recent.clone().into_iter().enumerate() {
                let stem = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                row = row.child(
                    Button::new(("recent", i)).xsmall().ghost().label(stem).on_click(
                        cx.listener(move |this, _, window, cx| {
                            match std::fs::read_to_string(&path) {
                                Ok(text) => {
                                    let b = this.active;
                                    this.buffers[b]
                                        .update(cx, |s, cx| s.set_value(text, window, cx));
                                    this.remember_file(b, &path);
                                    this.log.push(LogLine::info(format!(
                                        "→ Opened {} into buffer {}",
                                        path.display(),
                                        b + 1
                                    )));
                                }
                                Err(e) => this.log.push(LogLine::error(format!(
                                    "Open {} failed: {e}",
                                    path.display()
                                ))),
                            }
                            cx.notify();
                        }),
                    ),
                );
            }
        }
        row
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
    fn spectrum_el(&self, cx: &App) -> AnyElement {
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
    fn scope(&self, cx: &App) -> (AnyElement, bool) {
        let live = self.scope_live && !self.scope_samples.is_empty();
        if self.scope_mode == ScopeMode::Spectrum && live {
            return (self.spectrum_el(cx), true);
        }
        let color = cx.theme().primary;
        let right_color = run_color(3); // palette green — distinct from primary
        let mode = self.scope_mode;

        let (left, right) = if live {
            let l = self.scope_samples.clone();
            let r = if self.scope_right.len() == l.len() {
                self.scope_right.clone()
            } else {
                l.clone()
            };
            (l, r)
        } else {
            // Demo signal: sine left, cosine right (a circle in Lissajous).
            let n = 128usize;
            let phase = self.phase;
            let wave = |f: fn(f32) -> f32| {
                (0..n)
                    .map(|i| {
                        let t = i as f32 / n as f32;
                        f((t * std::f32::consts::TAU * 3.0) + phase) * (1.0 - t).max(0.0)
                    })
                    .collect::<Vec<f32>>()
            };
            (wave(f32::sin), wave(f32::cos))
        };

        /// One waveform trace as vertical bars around a midline.
        fn trace(
            window: &mut Window,
            samples: &[f32],
            x0: f32,
            bw: f32,
            mid: f32,
            half: f32,
            color: Hsla,
        ) {
            let bar_w = (bw / samples.len() as f32).max(1.0);
            for (i, s) in samples.iter().enumerate() {
                let h = (s.abs() * half).max(1.0);
                let x = x0 + i as f32 * bar_w;
                let y = if *s >= 0.0 { mid - h } else { mid };
                let bar = Bounds {
                    origin: point(px(x), px(y)),
                    size: size(px((bar_w * 0.8).max(1.0)), px(h)),
                };
                window.paint_quad(fill(bar, color));
            }
        }

        let el = div()
            .w_full()
            .h(px(150.))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, _| {
                        if left.is_empty() {
                            return;
                        }
                        let bw = f32::from(bounds.size.width);
                        let bh = f32::from(bounds.size.height);
                        let x0 = f32::from(bounds.origin.x);
                        let y0 = f32::from(bounds.origin.y);
                        match mode {
                            ScopeMode::Mono | ScopeMode::Spectrum => {
                                // Spectrum lands here while no data has flowed
                                // yet — draw the waveform demo instead.
                                trace(window, &left, x0, bw, y0 + bh / 2.0, bh / 2.0, color);
                            }
                            ScopeMode::Stereo => {
                                // Two half-height traces: left on top.
                                trace(window, &left, x0, bw, y0 + bh * 0.25, bh * 0.25, color);
                                trace(
                                    window, &right, x0, bw, y0 + bh * 0.75, bh * 0.25,
                                    right_color,
                                );
                            }
                            ScopeMode::Mirror => {
                                // Left rises from the midline, right mirrors below.
                                let mid = y0 + bh / 2.0;
                                let bar_w = (bw / left.len() as f32).max(1.0);
                                for i in 0..left.len() {
                                    let x = x0 + i as f32 * bar_w;
                                    let w = (bar_w * 0.8).max(1.0);
                                    let lh = (left[i].abs() * bh / 2.0).max(1.0);
                                    window.paint_quad(fill(
                                        Bounds {
                                            origin: point(px(x), px(mid - lh)),
                                            size: size(px(w), px(lh)),
                                        },
                                        color,
                                    ));
                                    let rh =
                                        (right.get(i).copied().unwrap_or(0.0).abs() * bh / 2.0)
                                            .max(1.0);
                                    window.paint_quad(fill(
                                        Bounds {
                                            origin: point(px(x), px(mid)),
                                            size: size(px(w), px(rh)),
                                        },
                                        right_color,
                                    ));
                                }
                            }
                            ScopeMode::Lissajous => {
                                // X = left sample, Y = right sample, dot per frame.
                                let (cx_, cy) = (x0 + bw / 2.0, y0 + bh / 2.0);
                                let scale = bw.min(bh) / 2.0;
                                for i in 0..left.len() {
                                    let sx = cx_ + left[i].clamp(-1.0, 1.0) * scale;
                                    let sy =
                                        cy - right.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0)
                                            * scale;
                                    window.paint_quad(fill(
                                        Bounds {
                                            origin: point(px(sx), px(sy)),
                                            size: size(px(2.0), px(2.0)),
                                        },
                                        color,
                                    ));
                                }
                            }
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element();
        (el, live)
    }
}

// ── Dock panels: scope/cues/log as Panel entities (drag tabs to rearrange) ──
//
// Read-only views over SonicSpike state: each observes the app entity so it
// re-renders on the 30fps tick, and reads state during render (immutable —
// no listener re-plumbing needed for these three).

macro_rules! oxide_panel {
    ($name:ident, $panel_id:literal, $title:literal) => {
        struct $name {
            app: Entity<SonicSpike>,
            focus: FocusHandle,
        }

        impl $name {
            fn new(app: Entity<SonicSpike>, cx: &mut Context<Self>) -> Self {
                cx.observe(&app, |_, _, cx| cx.notify()).detach();
                let focus = cx.focus_handle();
                Self { app, focus }
            }
        }

        impl EventEmitter<PanelEvent> for $name {}

        impl Focusable for $name {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus.clone()
            }
        }

        impl Panel for $name {
            fn panel_name(&self) -> &'static str {
                $panel_id
            }

            fn tab_name(&self, cx: &App) -> Option<SharedString> {
                Some(SharedString::from(self.app.read(cx).i18n.tr($title).to_string()))
            }

            fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                SharedString::from(self.app.read(cx).i18n.tr($title).to_string())
            }

            // The three core panes have no re-open affordance yet — keep the
            // tab's ✕ away until a "View" menu exists.
            fn closable(&self, _cx: &App) -> bool {
                false
            }
        }
    };
}

oxide_panel!(ScopePanel, "scope", "Scope");
oxide_panel!(CuesPanel, "cues", "Cues");
oxide_panel!(LogPanel, "log", "Log");

impl Render for ScopePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (el, live) = self.app.read(cx).scope(cx);
        div()
            .id("scope-panel")
            .role(Role::Image)
            .aria_label(if live { "Audio scope waveform (live)" } else { "Audio scope waveform" })
            .size_full()
            .overflow_hidden()
            .child(el)
    }
}

/// Thin per-pane action row: an xsmall "⧉ Copy All" button, usable while the
/// pane's content is being rewritten live (selection-based copying isn't).
fn copy_all_row(
    id: &'static str,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_end()
        .px_1()
        .child(Button::new(id).xsmall().ghost().label(label).on_click(on_click))
}

impl Render for CuesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let copy_label = format!("⧉ {}", app.i18n.tr("Copy All"));
        let cues = app.cues.clone();
        let label: SharedString = format!("Cues log. {}", cues.read(cx).value()).into();
        v_flex()
            .id("cues-panel")
            .role(Role::Group)
            .aria_label(label)
            .size_full()
            .overflow_hidden()
            .child(copy_all_row(
                "cues-copy-all",
                copy_label,
                cx.listener(|this, _, _, cx| {
                    let text = this.app.read(cx).cues.read(cx).value().to_string();
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }),
            ))
            .child(div().flex_1().min_h(px(0.)).child(Input::new(&cues).h_full()))
    }
}

impl Render for LogPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let show_debug = app.show_debug;
        let copy_label = format!("⧉ {}", app.i18n.tr("Copy All"));
        // Newest-first: the latest line is always visible without scrolling
        // (mid-performance), and the whole scrollback is reachable below.
        let rows: Vec<LogLine> =
            app.log.iter().rev().filter(|l| show_debug || !l.debug).cloned().collect();
        let label: SharedString = format!(
            "Run log. {}",
            rows.iter().take(16).map(|l| l.text.as_str()).collect::<Vec<_>>().join(". ")
        )
        .into();
        let log_fg = cx.theme().foreground;
        let log_err = cx.theme().danger;
        v_flex()
            .id("log-panel")
            .role(Role::Group)
            .aria_label(label)
            .size_full()
            .overflow_hidden()
            .child(copy_all_row(
                "log-copy-all",
                copy_label,
                cx.listener(|this, _, _, cx| {
                    // Clipboard gets chronological order (paste-friendly).
                    let app = this.app.read(cx);
                    let show_debug = app.show_debug;
                    let text: String = app
                        .log
                        .iter()
                        .filter(|l| show_debug || !l.debug)
                        .map(|l| l.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }),
            ))
            .child(
                v_flex()
                    .id("log-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_1()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .children(rows.into_iter().map(move |l| {
                        let color =
                            if l.error { log_err } else { l.run.map(run_color).unwrap_or(log_fg) };
                        let row = div().text_color(color);
                        let row = if l.indent { row.pl_4() } else { row };
                        row.child(l.text)
                    })),
            )
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

        let code = editor.read(cx).value();
        let caret = editor.read(cx).cursor();
        let editor_label: SharedString = format!(
            "Sonic Pi code editor, buffer {}, {} characters, cursor at offset {}",
            self.active + 1,
            code.len(),
            caret
        )
        .into();

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
                    .text_size(px(self.font_size))
                    .into_any_element(),
            ));

        let settings_el = self.settings_open.then(|| self.settings(cx));
        let help_el = self.help_open.then(|| self.help(cx));
        let tutorial_el = self.tutorial_open.then(|| self.tutorial(cx));
        let nodes_el = self.nodes_open.then(|| self.nodes(cx));
        let mut right = v_flex().size_full().gap_2().p_2();
        if let Some(el) = tutorial_el {
            right = right.child(self.pane(
                "tutorial",
                self.i18n.tr("Tutorial"),
                Role::Group,
                "Tutorial and examples browser",
                0.0,
                el,
                cx,
            ));
        }
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
        // The permanent panes live in the DockArea: resize via dividers,
        // drag tabs to rearrange/stack (the full dock story).
        let right = if let Some(dock) = &self.dock {
            right.child(div().flex_1().min_h(px(0.)).child(dock.clone()))
        } else {
            right
        };

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
            .on_action(
                cx.listener(|this, _: &OpenFile, window, cx| this.open_file(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SaveFile, window, cx| this.save_file(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SaveFileAs, window, cx| this.save_file(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(1.0, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(-1.0, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom(0.0, cx)))
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
        // Theme catalogue: etc/themes/*.json joins the registry (async), then
        // the persisted pick re-applies. watch_dir also live-reloads edits.
        let saved_theme = store::load_prefs(&store::default_store_dir()).get("theme").cloned();
        let _ = gpui_component::ThemeRegistry::watch_dir(
            app_root().join("../etc/themes"),
            cx,
            move |cx| {
                let Some(name) = saved_theme.clone() else { return };
                let cfg = gpui_component::ThemeRegistry::global(cx)
                    .themes()
                    .get(&SharedString::from(name))
                    .cloned();
                if let Some(cfg) = cfg {
                    gpui_component::Theme::global_mut(cx).apply_config(&cfg);
                    gpui_component::Theme::change(cfg.mode, None, cx);
                    cx.refresh_windows();
                }
            },
        );
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
            // Editor built-ins (Tier 1.5 audit): gpui-component's Input context
            // already binds ctrl-z undo / ctrl-y redo / ctrl-f find+replace
            // (registered by gpui_component::init; code_editor() sets
            // searchable). Add the conventional Linux redo alias it lacks:
            KeyBinding::new("ctrl-shift-z", gpui_component::input::Redo, Some("Input")),
            KeyBinding::new("ctrl-o", OpenFile, None),
            KeyBinding::new("ctrl-s", SaveFile, None),
            KeyBinding::new("ctrl-shift-s", SaveFileAs, None),
            KeyBinding::new("ctrl-=", ZoomIn, None),
            KeyBinding::new("ctrl--", ZoomOut, None),
            KeyBinding::new("ctrl-0", ZoomReset, None),
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
