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
    /// Option lines for synths/fx (`amp: 1`, `cutoff: 100`, …) — feeds the
    /// Help pane; empty for samples and functions.
    pub opts: Vec<String>,
}

impl VocabEntry {
    pub fn new(label: impl Into<String>, kind: VocabKind, doc: impl Into<String>) -> VocabEntry {
        VocabEntry { label: label.into(), kind, doc: doc.into(), opts: Vec::new() }
    }
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
    let mut opts: Vec<String> = Vec::new();
    #[derive(PartialEq)]
    enum Section {
        None,
        Doc,
        Opts,
    }
    let mut section = Section::None;

    let flush = |key: &mut Option<String>,
                 doc: &mut String,
                 opts: &mut Vec<String>,
                 out: &mut Vec<VocabEntry>| {
        if let Some(k) = key.take() {
            out.push(VocabEntry {
                label: k,
                kind,
                doc: std::mem::take(doc).trim().to_string(),
                opts: std::mem::take(opts),
            });
        } else {
            doc.clear();
            opts.clear();
        }
    };

    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            flush(&mut key, &mut doc, &mut opts, &mut entries);
            section = Section::None;
        } else if trimmed == "### Doc:" {
            section = Section::Doc;
        } else if trimmed == "### Opts:" {
            section = Section::Opts;
        } else if trimmed.starts_with("### ") {
            section = Section::None;
        } else if trimmed.starts_with(':') && key.is_none() {
            key = Some(trimmed.to_string());
        } else if section == Section::Doc && !trimmed.is_empty() {
            if !doc.is_empty() {
                doc.push(' ');
            }
            doc.push_str(trimmed);
        } else if section == Section::Opts && !trimmed.is_empty() {
            opts.push(trimmed.trim_start_matches('*').trim().to_string());
        }
    }
    flush(&mut key, &mut doc, &mut opts, &mut entries);
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
                    entries.push(VocabEntry::new(prev, VocabKind::Func, ""));
                }
                pending = Some(name);
            }
        } else if let Some(rest) = trimmed.strip_prefix("summary:") {
            if let Some(name) = pending.take() {
                let doc = rest.trim().trim_matches('"').trim_end_matches("\",").to_string();
                entries.push(VocabEntry::new(name, VocabKind::Func, doc));
            }
        }
    }
    if let Some(name) = pending.take() {
        entries.push(VocabEntry::new(name, VocabKind::Func, ""));
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
                out.push(VocabEntry::new(format!(":{stem}"), VocabKind::Sample, ""));
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

impl Vocab {
    /// Context-aware opt completion: when the text before the caret
    /// references a synth/fx symbol we know (`use_synth :tb303`,
    /// `with_fx :reverb, …` — possibly lines earlier, matching Sonic Pi's
    /// use_synth-persists semantics), offer that entry's opts (`cutoff:`,
    /// `mix:`, …) matching `prefix`. The *last* known symbol wins (closest
    /// context). Returns (insert label like `"cutoff:"`, default value).
    pub fn opt_completions(
        &self,
        text_before_caret: &str,
        prefix: &str,
        limit: usize,
    ) -> Vec<(String, String)> {
        if prefix.starts_with(':') || prefix.is_empty() {
            return Vec::new();
        }
        // Last :symbol before the caret that resolves to an entry with opts.
        let entry = text_before_caret
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
            .filter(|w| w.starts_with(':') && w.len() > 1 && !w.contains("::"))
            .filter_map(|w| self.get(w))
            .filter(|e| !e.opts.is_empty())
            .next_back();
        let Some(entry) = entry else {
            return Vec::new();
        };
        let mut out: Vec<(String, String)> = entry
            .opts
            .iter()
            .filter_map(|opt| {
                let (name, default) = opt.split_once(':')?;
                let name = name.trim();
                name.starts_with(prefix)
                    .then(|| (format!("{name}:"), default.trim().to_string()))
            })
            .collect();
        out.truncate(limit);
        out
    }

    /// Case-insensitive substring search for the Help pane.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&VocabEntry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<&VocabEntry> =
            self.entries.iter().filter(|e| e.label.to_lowercase().contains(&q)).collect();
        // Earlier match position first, then shorter label.
        out.sort_by_key(|e| (e.label.to_lowercase().find(&q).unwrap_or(usize::MAX), e.label.len()));
        out.truncate(limit);
        out
    }

    pub fn get(&self, label: &str) -> Option<&VocabEntry> {
        self.entries.iter().find(|e| e.label == label)
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
        assert_eq!(entries[0].opts, vec!["amp: 1"]);
        assert_eq!(entries[1].label, ":tb303");
        assert_eq!(entries[1].doc, "Emulation of the classic acid bass machine.");
        assert!(entries[1].opts.is_empty());
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
                VocabEntry::new(":tb303", VocabKind::Synth, ""),
                VocabEntry::new(":bd_haus", VocabKind::Sample, ""),
                VocabEntry::new("play", VocabKind::Func, ""),
                VocabEntry::new("play_chord", VocabKind::Func, ""),
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
    fn opt_completions_follow_the_nearest_symbol() {
        let mut tb303 = VocabEntry::new(":tb303", VocabKind::Synth, "");
        tb303.opts = vec!["cutoff: 100".into(), "res: 0.9".into(), "amp: 1".into()];
        let mut reverb = VocabEntry::new(":reverb", VocabKind::Fx, "");
        reverb.opts = vec!["mix: 0.4".into(), "room: 0.6".into()];
        let vocab = Vocab {
            entries: vec![tb303, reverb, VocabEntry::new(":bd_haus", VocabKind::Sample, "")],
        };

        // Opts of the line's synth, prefix-filtered, with defaults.
        let hits = vocab.opt_completions("use_synth :tb303\nplay 60, cu", "cu", 10);
        assert_eq!(hits, vec![("cutoff:".to_string(), "100".to_string())]);

        // Two symbols on the line: the closest (last) one wins.
        let hits = vocab.opt_completions("with_fx :reverb do :tb303 ", "re", 10);
        assert_eq!(hits, vec![("res:".to_string(), "0.9".to_string())]);

        // Symbols without opts (samples) contribute nothing.
        assert!(vocab.opt_completions("sample :bd_haus, am", "am", 10).is_empty());
        // Symbol prefixes never get opts.
        assert!(vocab.opt_completions("use_synth :tb303, ", ":cu", 10).is_empty());
    }

    #[test]
    fn search_is_substring_and_ranked() {
        let vocab = Vocab {
            entries: vec![
                VocabEntry::new(":bd_haus", VocabKind::Sample, ""),
                VocabEntry::new("play", VocabKind::Func, ""),
                VocabEntry::new("play_pattern_timed", VocabKind::Func, ""),
            ],
        };
        let hits: Vec<_> = vocab.search("haus", 10).iter().map(|e| e.label.clone()).collect();
        assert_eq!(hits, vec![":bd_haus"]);
        // Prefix matches rank before mid-string; shorter first on ties.
        let hits: Vec<_> = vocab.search("play", 10).iter().map(|e| e.label.clone()).collect();
        assert_eq!(hits, vec!["play", "play_pattern_timed"]);
        assert!(vocab.search("", 10).is_empty());
        assert_eq!(vocab.get("play").unwrap().label, "play");
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
