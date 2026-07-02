//! Sonic Pi completion vocabulary, loaded from the repo's own sources of
//! truth (the same ones the docs are generated from):
//!
//!   * synths + fx — `etc/doc/cheatsheets/{synths,fx}.md` (`### Key:` blocks
//!     with `### Doc:` summaries)
//!   * samples     — the samples directory listing (live), grouped names from
//!     `cheatsheets/samples.md` kept as the doc fallback
//!   * functions   — `app/server/ruby/lib/sonicpi/lang/*.rb` `doc name:`
//!     blocks with their `summary:` lines (the exact data the in-app help
//!     shows)
//!
//! Pure parsing + matching — unit-tested without any runtime.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabKind {
    Synth,
    Fx,
    Sample,
    Func,
}

impl VocabKind {
    pub fn label(&self) -> &'static str {
        match self {
            VocabKind::Synth => "synth",
            VocabKind::Fx => "fx",
            VocabKind::Sample => "sample",
            VocabKind::Func => "fn",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VocabEntry {
    /// What completion inserts: `:tb303`, `:bd_haus`, `play`, …
    pub label: String,
    pub kind: VocabKind,
    /// One-line summary shown alongside the item.
    pub doc: String,
}

pub struct Vocab {
    pub entries: Vec<VocabEntry>,
}

/// Parse a `synths.md`/`fx.md` cheatsheet: `## Title` sections holding
/// `### Key:\n  :name` and `### Doc:\n  summary…` blocks.
pub fn parse_cheatsheet(md: &str, kind: VocabKind) -> Vec<VocabEntry> {
    let mut entries = Vec::new();
    let mut key: Option<String> = None;
    let mut doc = String::new();
    let mut in_doc = false;

    let flush = |key: &mut Option<String>, doc: &mut String, out: &mut Vec<VocabEntry>| {
        if let Some(k) = key.take() {
            out.push(VocabEntry { label: k, kind, doc: std::mem::take(doc).trim().to_string() });
        } else {
            doc.clear();
        }
    };

    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            flush(&mut key, &mut doc, &mut entries);
            in_doc = false;
        } else if trimmed == "### Key:" {
            in_doc = false;
        } else if trimmed == "### Doc:" {
            in_doc = true;
        } else if trimmed.starts_with("### ") {
            in_doc = false;
        } else if trimmed.starts_with(':') && key.is_none() {
            key = Some(trimmed.to_string());
        } else if in_doc && !trimmed.is_empty() {
            if !doc.is_empty() {
                doc.push(' ');
            }
            doc.push_str(trimmed);
        }
    }
    flush(&mut key, &mut doc, &mut entries);
    entries
}

/// Extract `doc name: :fn` + `summary: "…"` pairs from a Sonic Pi lang
/// source file (core.rb / sound.rb / midi.rb / …).
pub fn parse_lang_docs(rb: &str) -> Vec<VocabEntry> {
    let mut entries = Vec::new();
    let mut pending: Option<String> = None;

    for line in rb.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("doc name:") {
            let name: String = rest
                .trim_start()
                .strip_prefix(':')
                .unwrap_or("")
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '?' || *c == '!')
                .collect();
            if !name.is_empty() {
                // Flush a previous block that never found its summary.
                if let Some(prev) = pending.take() {
                    entries.push(VocabEntry { label: prev, kind: VocabKind::Func, doc: String::new() });
                }
                pending = Some(name);
            }
        } else if let Some(rest) = trimmed.strip_prefix("summary:") {
            if let Some(name) = pending.take() {
                let doc = rest.trim().trim_matches('"').trim_end_matches("\",").to_string();
                entries.push(VocabEntry { label: name, kind: VocabKind::Func, doc });
            }
        }
    }
    if let Some(name) = pending.take() {
        entries.push(VocabEntry { label: name, kind: VocabKind::Func, doc: String::new() });
    }
    entries
}

/// Sample names from the samples directory (`:file_stem` per audio file).
pub fn samples_from_dir(dir: &Path) -> Vec<VocabEntry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(ext, "flac" | "wav" | "wave" | "aif" | "aiff") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                out.push(VocabEntry {
                    label: format!(":{stem}"),
                    kind: VocabKind::Sample,
                    doc: String::new(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

impl Vocab {
    /// Load everything relative to the repo layout (`app_root` = the `app/`
    /// dir the spike already resolves paths from). Missing files degrade to
    /// an empty section, never an error — completion just knows less.
    pub fn load(app_root: &Path) -> Vocab {
        let repo = app_root.join("..");
        let cheat = |name: &str| {
            std::fs::read_to_string(repo.join("etc/doc/cheatsheets").join(name))
                .unwrap_or_default()
        };
        let mut entries = parse_cheatsheet(&cheat("synths.md"), VocabKind::Synth);
        entries.extend(parse_cheatsheet(&cheat("fx.md"), VocabKind::Fx));
        entries.extend(samples_from_dir(&repo.join("etc/samples")));

        let lang_dir = app_root.join("server/ruby/lib/sonicpi/lang");
        if let Ok(rd) = std::fs::read_dir(&lang_dir) {
            for entry in rd.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("rb") {
                    if let Ok(src) = std::fs::read_to_string(entry.path()) {
                        entries.extend(parse_lang_docs(&src));
                    }
                }
            }
        }

        entries.sort_by(|a, b| a.label.cmp(&b.label));
        entries.dedup_by(|a, b| a.label == b.label && a.kind == b.kind);
        Vocab { entries }
    }

    /// Prefix-match against the current word. A `:`-prefix restricts to
    /// symbols (synths/fx/samples); a bare identifier matches functions first,
    /// then symbol bodies (so `bd_h` still finds `:bd_haus`).
    pub fn complete(&self, prefix: &str, limit: usize) -> Vec<&VocabEntry> {
        if prefix.len() < 2 {
            return Vec::new();
        }
        let mut out: Vec<&VocabEntry> = Vec::new();
        if let Some(sym) = prefix.strip_prefix(':') {
            out.extend(
                self.entries
                    .iter()
                    .filter(|e| e.kind != VocabKind::Func)
                    .filter(|e| e.label[1..].starts_with(sym)),
            );
        } else {
            out.extend(
                self.entries
                    .iter()
                    .filter(|e| e.kind == VocabKind::Func)
                    .filter(|e| e.label.starts_with(prefix)),
            );
            out.extend(
                self.entries
                    .iter()
                    .filter(|e| e.kind != VocabKind::Func)
                    .filter(|e| e.label[1..].starts_with(prefix)),
            );
        }
        out.truncate(limit);
        out
    }
}

/// The word being typed at `offset` (byte) — identifier chars plus a leading
/// `:` for symbols. Returns (start_byte, word).
pub fn word_prefix_at(text: &str, offset: usize) -> (usize, &str) {
    let offset = {
        let mut o = offset.min(text.len());
        while o > 0 && !text.is_char_boundary(o) {
            o -= 1;
        }
        o
    };
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 {
        let c = bytes[start - 1] as char;
        if c.is_ascii_alphanumeric() || c == '_' {
            start -= 1;
        } else {
            break;
        }
    }
    // Include one leading ':' (symbol prefix).
    if start > 0 && bytes[start - 1] as char == ':' {
        // ...but not '::' (scope operator).
        if !(start > 1 && bytes[start - 2] as char == ':') {
            start -= 1;
        }
    }
    (start, &text[start..offset])
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHEAT: &str = "# Synths\n\n* [Dull Bell](#dull-bell)\n\n## Dull Bell\n\n### Key:\n  :dull_bell\n\n### Doc:\n  A simple dull discordant bell sound.\n\n### Opts:\n  * amp: 1\n\n## TB-303 Emulation\n\n### Key:\n  :tb303\n\n### Doc:\n  Emulation of the classic\n  acid bass machine.\n";

    #[test]
    fn parses_cheatsheet_keys_and_docs() {
        let entries = parse_cheatsheet(CHEAT, VocabKind::Synth);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, ":dull_bell");
        assert_eq!(entries[0].doc, "A simple dull discordant bell sound.");
        assert_eq!(entries[1].label, ":tb303");
        assert_eq!(entries[1].doc, "Emulation of the classic acid bass machine.");
    }

    #[test]
    fn parses_lang_doc_blocks() {
        let rb = r#"
       doc name:          :play,
           introduced:    Version.new(2,0,0),
           summary:       "Play current synth",
           doc:           "long text"
       doc name:          :live_loop,
           summary:       "A loop for live coding",
"#;
        let entries = parse_lang_docs(rb);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "play");
        assert_eq!(entries[0].doc, "Play current synth");
        assert_eq!(entries[1].label, "live_loop");
    }

    #[test]
    fn completes_symbols_and_functions() {
        let vocab = Vocab {
            entries: vec![
                VocabEntry { label: ":tb303".into(), kind: VocabKind::Synth, doc: String::new() },
                VocabEntry { label: ":bd_haus".into(), kind: VocabKind::Sample, doc: String::new() },
                VocabEntry { label: "play".into(), kind: VocabKind::Func, doc: String::new() },
                VocabEntry { label: "play_chord".into(), kind: VocabKind::Func, doc: String::new() },
            ],
        };
        // Symbol prefix restricts to symbols.
        let hits: Vec<_> = vocab.complete(":tb", 10).iter().map(|e| e.label.clone()).collect();
        assert_eq!(hits, vec![":tb303"]);
        // Bare identifier: functions first, then symbol bodies.
        let hits: Vec<_> = vocab.complete("pla", 10).iter().map(|e| e.label.clone()).collect();
        assert_eq!(hits, vec!["play", "play_chord"]);
        let hits: Vec<_> = vocab.complete("bd_", 10).iter().map(|e| e.label.clone()).collect();
        assert_eq!(hits, vec![":bd_haus"]);
        // Too short → nothing.
        assert!(vocab.complete("p", 10).is_empty());
    }

    #[test]
    fn loads_real_repo_vocabulary() {
        // Integration against the actual repo files this spike ships with.
        let app_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
        let vocab = Vocab::load(&app_root);
        let labels: Vec<&str> = vocab.entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&":tb303"), "synths missing");
        assert!(labels.contains(&":bd_haus"), "samples missing");
        assert!(labels.contains(&"live_loop"), "lang functions missing");
        assert!(vocab.entries.len() > 300, "suspiciously small vocab: {}", vocab.entries.len());
        // fx cheatsheet keys are :fx_* — completion after with_fx types ':'
        assert!(labels.iter().any(|l| l.starts_with(":fx_") || l.starts_with(":reverb")), "fx missing");
    }

    #[test]
    fn word_prefix_extraction() {
        assert_eq!(word_prefix_at("play :tb3", 9), (5, ":tb3"));
        assert_eq!(word_prefix_at("  live_lo", 9), (2, "live_lo"));
        assert_eq!(word_prefix_at("play 60", 4), (0, "play"));
        assert_eq!(word_prefix_at("x = A::B", 8), (7, "B"));
        assert_eq!(word_prefix_at("", 0), (0, ""));
    }
}
