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
    Note,
    Scale,
    Chord,
}

impl VocabKind {
    pub const ALL: [VocabKind; 7] = [
        VocabKind::Synth,
        VocabKind::Fx,
        VocabKind::Sample,
        VocabKind::Func,
        VocabKind::Note,
        VocabKind::Scale,
        VocabKind::Chord,
    ];

    /// Category-browser button label (i18n key).
    pub fn plural(&self) -> &'static str {
        match self {
            VocabKind::Synth => "Synths",
            VocabKind::Fx => "FX",
            VocabKind::Sample => "Samples",
            VocabKind::Func => "Functions",
            VocabKind::Note => "Notes",
            VocabKind::Scale => "Scales",
            VocabKind::Chord => "Chords",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            VocabKind::Synth => "synth",
            VocabKind::Fx => "fx",
            VocabKind::Sample => "sample",
            VocabKind::Func => "fn",
            VocabKind::Note => "note",
            VocabKind::Scale => "scale",
            VocabKind::Chord => "chord",
        }
    }
}

/// One synth/fx option, fully documented (mirrors the cheatsheet detail the
/// Qt help renders: name, default, description, constraint/modulation notes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptDoc {
    /// Insertable name, colon included: `"cutoff:"`.
    pub name: String,
    pub default: String,
    pub doc: String,
    /// Constraint / modulation notes ("must be zero or greater", "Has slide
    /// parameters…").
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct VocabEntry {
    /// What completion inserts: `:tb303`, `:bd_haus`, `play`, …
    pub label: String,
    pub kind: VocabKind,
    /// One-line summary shown alongside the item.
    pub doc: String,
    /// Documented options for synths/fx — feeds completions and the Help
    /// pane; empty for samples and functions.
    pub opt_docs: Vec<OptDoc>,
    /// Full documentation body (the lang `doc:` field) — Help-pane only;
    /// empty when the source has no long doc.
    pub long_doc: String,
}

impl VocabEntry {
    pub fn new(label: impl Into<String>, kind: VocabKind, doc: impl Into<String>) -> VocabEntry {
        VocabEntry {
            label: label.into(),
            kind,
            doc: doc.into(),
            opt_docs: Vec::new(),
            long_doc: String::new(),
        }
    }

    /// The entry as a markdown document — the Help pane's detail view
    /// (title, summary, full doc body, and a per-option reference like the
    /// Qt help's synth/fx pages). Pure → unit-tested.
    pub fn doc_markdown(&self) -> String {
        let mut md = format!("## `{}` — {}\n\n", self.label, self.kind.label());
        if !self.doc.is_empty() {
            md.push_str(&format!("{}\n\n", self.doc));
        }
        if !self.long_doc.is_empty() {
            md.push_str(&format!("{}\n\n", self.long_doc));
        }
        if !self.opt_docs.is_empty() {
            md.push_str("### Options\n\n");
            for o in &self.opt_docs {
                md.push_str(&format!("**`{}`**", o.name));
                if !o.default.is_empty() {
                    md.push_str(&format!(" *(default {})*", o.default));
                }
                if !o.doc.is_empty() {
                    md.push_str(&format!(" — {}", o.doc));
                }
                md.push('\n');
                if !o.notes.is_empty() {
                    md.push_str(&format!("\n  _{}_\n", o.notes.join("; ")));
                }
                md.push('\n');
            }
        }
        md
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
    let mut opts: Vec<OptDoc> = Vec::new();
    #[derive(PartialEq)]
    enum Section {
        None,
        Doc,
        Opts,
    }
    let mut section = Section::None;

    let flush = |key: &mut Option<String>,
                 doc: &mut String,
                 opts: &mut Vec<OptDoc>,
                 out: &mut Vec<VocabEntry>| {
        if let Some(k) = key.take() {
            out.push(VocabEntry {
                label: k,
                kind,
                doc: std::mem::take(doc).trim().to_string(),
                opt_docs: std::mem::take(opts),
                long_doc: String::new(),
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
            // `* name:` opens an option; its `- doc:` / `- default:` /
            // `- <note>` sub-lines fill it in. (A legacy `* name: default`
            // one-liner still parses.)
            if let Some(rest) = trimmed.strip_prefix('*') {
                let rest = rest.trim();
                let (name, default) = match rest.split_once(':') {
                    Some((n, d)) => (format!("{}:", n.trim()), d.trim().to_string()),
                    None => (rest.to_string(), String::new()),
                };
                opts.push(OptDoc { name, default, ..Default::default() });
            } else if let Some(rest) = trimmed.strip_prefix('-') {
                if let Some(o) = opts.last_mut() {
                    let rest = rest.trim();
                    if let Some(d) = rest.strip_prefix("doc:") {
                        o.doc = d.trim().to_string();
                    } else if let Some(d) = rest.strip_prefix("default:") {
                        o.default = d.trim().to_string();
                    } else {
                        o.notes.push(rest.to_string());
                    }
                }
            }
        }
    }
    flush(&mut key, &mut doc, &mut opts, &mut entries);
    entries
}

/// Extract `doc name: :fn` + `summary: "…"` + multiline `doc: "…"` blocks
/// from a Sonic Pi lang source file (core.rb / sound.rb / midi.rb / …).
pub fn parse_lang_docs(rb: &str) -> Vec<VocabEntry> {
    let mut entries: Vec<VocabEntry> = Vec::new();
    let mut pending: Option<String> = None;
    // (entry index, accumulated text) while inside a multiline doc: string.
    let mut capturing: Option<(usize, String)> = None;

    // True when a double-quoted Ruby string closes on this fragment.
    fn closes(fragment: &str) -> Option<usize> {
        let bytes = fragment.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 1, // skip the escaped char
                b'"' => return Some(i),
                _ => {}
            }
            i += 1;
        }
        None
    }

    for line in rb.lines() {
        if let Some((ix, mut text)) = capturing.take() {
            match closes(line) {
                Some(end) => {
                    text.push('\n');
                    text.push_str(&line[..end]);
                    entries[ix].long_doc = text.replace("\\\"", "\"").trim().to_string();
                }
                None => {
                    text.push('\n');
                    text.push_str(line);
                    capturing = Some((ix, text));
                }
            }
            continue;
        }

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
        } else if let Some(rest) = trimmed.strip_prefix("doc:") {
            // Attach the long doc to the most recent entry.
            let Some(ix) = entries.len().checked_sub(1) else { continue };
            let Some(start) = rest.find('"') else { continue };
            let body = &rest[start + 1..];
            match closes(body) {
                Some(end) => {
                    entries[ix].long_doc =
                        body[..end].replace("\\\"", "\"").trim().to_string();
                }
                None => capturing = Some((ix, body.to_string())),
            }
        }
    }
    if let Some(name) = pending.take() {
        entries.push(VocabEntry::new(name, VocabKind::Func, ""));
    }
    entries
}

/// All note symbols (`:c4`, `:fs3`, `:bb5`, …) — the piano-helper vocabulary,
/// generated rather than parsed (the pitch grammar is fixed).
pub fn note_names() -> Vec<VocabEntry> {
    const NAMES: [&str; 17] = [
        "c", "cs", "db", "d", "ds", "eb", "e", "f", "fs", "gb", "g", "gs", "ab", "a", "as", "bb",
        "b",
    ];
    let mut out = Vec::new();
    for octave in 0..=8 {
        for name in NAMES {
            out.push(VocabEntry::new(format!(":{name}{octave}"), VocabKind::Note, ""));
        }
    }
    out
}

/// Scale names from `scale.rb`'s SCALE hash (modern `name: value` syntax —
/// the keys of the first hash literal after `SCALE = lambda`).
pub fn parse_scale_names(rb: &str) -> Vec<VocabEntry> {
    let mut out = Vec::new();
    let Some(start) = rb.find("SCALE = lambda") else { return out };
    let Some(brace) = rb[start..].find("\n      {") else { return out };
    for line in rb[start + brace..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with('}') {
            break;
        }
        if let Some((key, _)) = trimmed.split_once(':') {
            let key = key.trim();
            if !key.is_empty()
                && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                out.push(VocabEntry::new(format!(":{key}"), VocabKind::Scale, ""));
            }
        }
    }
    out
}

/// Chord names from `chord.rb` (`:name =>` keys).
pub fn parse_chord_names(rb: &str) -> Vec<VocabEntry> {
    let mut out = Vec::new();
    for line in rb.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(':') else { continue };
        let Some((key, tail)) = rest.split_once(char::is_whitespace) else { continue };
        if tail.trim_start().starts_with("=>")
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            out.push(VocabEntry::new(format!(":{key}"), VocabKind::Chord, ""));
        }
    }
    out
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

        // Notes (generated) + scales/chords from their Ruby sources of truth.
        entries.extend(note_names());
        let sonicpi = app_root.join("server/ruby/lib/sonicpi");
        if let Ok(src) = std::fs::read_to_string(sonicpi.join("scale.rb")) {
            entries.extend(parse_scale_names(&src));
        }
        if let Ok(src) = std::fs::read_to_string(sonicpi.join("chord.rb")) {
            entries.extend(parse_chord_names(&src));
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
            .filter(|e| !e.opt_docs.is_empty())
            .next_back();
        let Some(entry) = entry else {
            return Vec::new();
        };
        let mut out: Vec<(String, String)> = entry
            .opt_docs
            .iter()
            .filter(|o| o.name.trim_end_matches(':').starts_with(prefix))
            .map(|o| (o.name.clone(), o.default.clone()))
            .collect();
        out.truncate(limit);
        out
    }

    /// Every entry of one kind, label-sorted — the Help pane's category
    /// browser (Qt help-tab parity).
    pub fn by_kind(&self, kind: VocabKind) -> Vec<&VocabEntry> {
        let mut out: Vec<&VocabEntry> = self.entries.iter().filter(|e| e.kind == kind).collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
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
        // Legacy one-liner opt: `* amp: 1`.
        assert_eq!(entries[0].opt_docs.len(), 1);
        assert_eq!(entries[0].opt_docs[0].name, "amp:");
        assert_eq!(entries[0].opt_docs[0].default, "1");
        assert_eq!(entries[1].label, ":tb303");
        assert_eq!(entries[1].doc, "Emulation of the classic acid bass machine.");
        assert!(entries[1].opt_docs.is_empty());
    }

    #[test]
    fn parses_real_format_opt_docs() {
        // The shipped cheatsheets document each opt with `- doc/default/…`
        // sub-lines — the format the Qt help renders.
        let md = "## Dull Bell\n\n### Key:\n  :dull_bell\n\n### Doc:\n  A bell.\n\n### Opts:\n  * note:\n    - doc: Note to play.\n    - default: 52\n    - constraints: must be zero or greater\n    - May be changed whilst playing\n  * amp:\n    - doc: The amplitude.\n    - default: 1\n";
        let entries = parse_cheatsheet(md, VocabKind::Synth);
        assert_eq!(entries.len(), 1);
        let opts = &entries[0].opt_docs;
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].name, "note:");
        assert_eq!(opts[0].default, "52");
        assert_eq!(opts[0].doc, "Note to play.");
        assert_eq!(
            opts[0].notes,
            vec!["constraints: must be zero or greater", "May be changed whilst playing"]
        );
        assert_eq!(opts[1].name, "amp:");
        assert_eq!(opts[1].default, "1");

        // Markdown detail view carries all of it, uncapped.
        let md = entries[0].doc_markdown();
        assert!(md.contains("`:dull_bell`"));
        assert!(md.contains("**`note:`** *(default 52)* — Note to play."));
        assert!(md.contains("must be zero or greater"));
    }

    #[test]
    fn by_kind_lists_sorted_categories() {
        let vocab = Vocab {
            entries: vec![
                VocabEntry::new(":tb303", VocabKind::Synth, ""),
                VocabEntry::new(":beep", VocabKind::Synth, ""),
                VocabEntry::new("play", VocabKind::Func, ""),
            ],
        };
        let synths: Vec<&str> =
            vocab.by_kind(VocabKind::Synth).iter().map(|e| e.label.as_str()).collect();
        assert_eq!(synths, vec![":beep", ":tb303"]);
        assert_eq!(vocab.by_kind(VocabKind::Sample).len(), 0);
    }

    #[test]
    fn parses_lang_doc_blocks() {
        let rb = r#"
       doc name:          :play,
           introduced:    Version.new(2,0,0),
           summary:       "Play current synth",
           doc:           "long text
spanning multiple lines with a \"quoted\" bit
and more.",
           examples:      ["play 60"]
       doc name:          :live_loop,
           summary:       "A loop for live coding",
           doc:           "one liner",
"#;
        let entries = parse_lang_docs(rb);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "play");
        assert_eq!(entries[0].doc, "Play current synth");
        assert_eq!(
            entries[0].long_doc,
            "long text\nspanning multiple lines with a \"quoted\" bit\nand more."
        );
        assert_eq!(entries[1].label, "live_loop");
        assert_eq!(entries[1].long_doc, "one liner");
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
        // Real cheatsheets use `- default:` sub-lines — a real synth must have
        // fully-documented opts (defaults were silently empty before the
        // structured OptDoc parse).
        let tb = vocab.get(":tb303").expect("tb303");
        let cutoff = tb.opt_docs.iter().find(|o| o.name == "cutoff:").expect("cutoff opt");
        assert!(!cutoff.default.is_empty(), "opt default lost");
        assert!(!cutoff.doc.is_empty(), "opt doc lost");
    }

    fn opt(name: &str, default: &str) -> OptDoc {
        OptDoc { name: name.into(), default: default.into(), ..Default::default() }
    }

    #[test]
    fn opt_completions_follow_the_nearest_symbol() {
        let mut tb303 = VocabEntry::new(":tb303", VocabKind::Synth, "");
        tb303.opt_docs = vec![opt("cutoff:", "100"), opt("res:", "0.9"), opt("amp:", "1")];
        let mut reverb = VocabEntry::new(":reverb", VocabKind::Fx, "");
        reverb.opt_docs = vec![opt("mix:", "0.4"), opt("room:", "0.6")];
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
    fn notes_scales_chords_load() {
        let notes = note_names();
        assert_eq!(notes.len(), 17 * 9);
        assert!(notes.iter().any(|n| n.label == ":c4"));
        assert!(notes.iter().any(|n| n.label == ":fs3"));

        let scale_rb = "    SCALE = lambda{\n      x = 1\n      {\n           diatonic:           ionian_sequence,\n           dorian:             ionian_sequence.rotate(1),\n      }\n    }.call\n";
        let scales: Vec<String> =
            parse_scale_names(scale_rb).into_iter().map(|e| e.label).collect();
        assert_eq!(scales, vec![":diatonic", ":dorian"]);

        let chord_rb = "        :major7          => major7,\n        :m7 => minor7,\n        not_a_chord\n";
        let chords: Vec<String> =
            parse_chord_names(chord_rb).into_iter().map(|e| e.label).collect();
        assert_eq!(chords, vec![":major7", ":m7"]);
    }

    #[test]
    fn real_repo_has_notes_scales_chords() {
        let app_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
        let vocab = Vocab::load(&app_root);
        let has = |l: &str, k: VocabKind| {
            vocab.entries.iter().any(|e| e.label == l && e.kind == k)
        };
        assert!(has(":c4", VocabKind::Note));
        assert!(has(":minor_pentatonic", VocabKind::Scale), "scale.rb parse broke");
        assert!(has(":major7", VocabKind::Chord), "chord.rb parse broke");
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
