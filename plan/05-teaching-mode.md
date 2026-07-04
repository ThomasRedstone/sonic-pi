# Phase 7 — Teaching mode: make the tutorial playable

> Read [`00-roadmap.md`](./00-roadmap.md) first.

## Goal

The tutorial browser (`experiments/gpui-spike/src/tutorial.rs`, wired into
the 📖 pane in `main.rs`) renders all 85 chapters as markdown with images —
but it's read-only. Sonic Pi's whole pedagogy is learn-by-sound: every code
block in the tutorial is meant to be run, heard, and modified. Close that
gap.

## Why this, why now

Sonic Pi is used to *teach* — schools, workshops, self-learners. A help
system you can only read is a worse teaching tool than the thing it's
replacing (Qt's tutorial pane has the same code-block-as-inert-text
limitation, so this is a chance to exceed parity, not just match it). It's
also cheap relative to its payoff: the loader, the markdown renderer, and
the run/load machinery all already exist; this phase mostly wires them
together.

## Design sketch

### 1. Runnable code blocks

`gpui-component`'s markdown view parses fenced code blocks (confirmed while
building the images patch — `text/node.rs` has a code-block node type
alongside the image node). Two options, in order of cheapness:

- **(a) Cheapest, ship first):** render code blocks as-is via the markdown
  view, but overlay a small "▶ Run" button next to each one (a click target
  positioned from the block's rendered bounds — the same `range_to_bounds`
  mechanism the glyph-metrics work already uses, so this isn't new
  infrastructure). Clicking extracts that block's raw text and calls the
  existing `run_active`-equivalent path against a **dedicated scratch
  buffer** (not the user's buffers 1-10 — never clobber their work), then
  flips to the Editor pane so they see it land somewhere real.
- **(b) Nicer, more work:** a custom markdown code-block renderer that swaps
  in an actual `InputState` editor for each block (editable in place, so a
  reader can tweak `sleep 0.5` to `sleep 0.25` and re-run without leaving
  the tutorial). This is more invasive — likely another gpui-component
  patch or a custom node type — so start with (a) and only reach for (b) if
  daily/teaching use shows people want to edit inline rather than "copy the
  idea into a real buffer," which the existing example-loader flow
  (`tutorial::Example` → load into active buffer) already sort of covers.

### 2. First-run experience

Right now a brand-new user lands on 10 empty-or-seeded buffers with no
signpost. Add:
- On first launch (no `prefs.conf` exists yet — the same signal
  `store::load_prefs` already distinguishes "never configured" from
  "configured with defaults"), auto-open the Tutorial pane to chapter 1.1
  and flash/highlight the 📖 button once.
- A "Start here" affordance inside the tutorial pane itself when no chapter
  is selected, replacing the current bare hint text with something that
  actually launches chapter 1.

### 3. Progress tracking (optional, cheap)

A `Vec<bool>` (or `HashSet<usize>`) of "chapters visited," persisted in
prefs.conf like everything else, surfaced as a checkmark next to visited
chapters in the nav list. No new infrastructure — `prefs` already round-trips
arbitrary key/value pairs.

## Status (2026-07-04)

✅ **Implemented.** `tutorial::extract_code_blocks` (pure, unit-tested
against every real chapter — 50+ chapters with fences all yield ≥1 block)
feeds a "Try it" strip of "▶ Example N" buttons beneath the chapter
markdown. Each click loads that block's text into a wholly separate
`scratch: Entity<InputState>` field (never one of the user's 10 numbered
buffers) and runs it through the existing, unmodified run pipeline — a
`current_editor()`/`current_run_name()` pair now generalizes
`run_active`/`comment_active`/`align_active` to operate on "whichever
buffer is showing" (scratch or numbered), which also fixed a latent
correctness gap: before this, Alt+/ and Align while viewing the scratch
buffer would have silently edited the wrong (numbered) buffer.

**Scope note vs. the original design sketch:** approach (a)'s "overlay a
Run button positioned via `range_to_bounds`" turned out not to fit —
`range_to_bounds` is an `InputState` (editor) API; the markdown `TextView`
that renders chapters doesn't expose per-code-block rendered bounds. Shipped
instead: a clean "Try it" strip listing every block in the chapter, below
the markdown rather than overlaid inline on it. Functionally equivalent
(every example gets a one-click Run), simpler, and needed no
gpui-component patch.

**First-run experience also done**: `first_run` is detected precisely
(`!store_dir.join("prefs.conf").exists()`, checked before anything can
create it) and eagerly loads + selects chapter 1 exactly as `Cmd::Tutorial`
would. The tutorial pane's empty state (no chapter picked, but chapters
exist) also grew a "▶ Start here" button. Verified end-to-end against a
real boot: a new `SONIC_OXIDE_STORE_DIR` env override (documented in
BUILD.md) points the whole workspace/prefs system at a throwaway directory
so this could be smoke-tested with the REAL runtime without touching Tom's
actual `~/.sonic-pi/store/sonic-oxide` — AUTOQUIT run PASSED (spider +
engine alive, clean shutdown, buffers correctly written to the throwaway
dir), proving the whole new code path (chapter load + extraction + first-
chapter selection, all running during real app construction) executes
without panicking.

**Not done (optional, per the original plan):** progress tracking
(checkmarks on visited chapters) — cheap or infra exists (prefs round-trip
arbitrary keys already), just not built; low priority next to the
higher-value phases.

## Work items

1. `tutorial.rs`: a pure function extracting fenced code blocks + their
   source ranges from a chapter's markdown (mirrors `absolutize_image_paths`'s
   shape — scan, don't parse the whole document with a new dependency).
   Unit test against real chapters (same pattern as the images real-repo
   test): every chapter with a ```code fence``` block should extract at
   least one runnable snippet.
2. `main.rs`: render the per-block Run button; wire it to a scratch buffer
   (`buffers.len()`-th slot, or a genuinely separate `Entity<InputState>`
   kept outside the 10-buffer array so it never collides with buffer
   indices elsewhere in the code — check `select_buffer`/`file_paths`
   assume a fixed `BUFFER_COUNT` before deciding which).
3. First-run detection + auto-open; "Start here" empty state.
4. (Optional) progress tracking.
5. i18n: any new user-visible strings go through `i18n.tr`, both locales
   updated, `python3 experiments/i18n-tools.py missing de/fr` clean.

## Exit criteria

- Every tutorial chapter's first code example runs with one click and
  produces audible output (or the expected visual/log effect for
  non-audio examples) without touching the user's own buffers.
- A brand-new profile (fresh `~/.sonic-pi/store/sonic-oxide`) opens
  straight into the tutorial with a clear next action, no blank screen.
- No regression in the existing example-loader flow (📖 → Examples →
  click → loads into active buffer) — that's a separate, still-useful path
  for browsing the repo's example library rather than the guided
  curriculum.

## Risks

- **Scratch-buffer bookkeeping**: make sure autosave/file-path logic
  (`remember_file`, `file_paths[i]`) can't be reached with an out-of-range
  index if the scratch buffer is bolted on carelessly. Prefer a genuinely
  separate field over overloading the existing `Vec<Entity<InputState>>`.
- **Markdown code-fence extraction drift**: if a chapter's example spans
  multiple fenced blocks meant to run together (rare but check), a
  block-at-a-time Run button could produce a misleadingly incomplete sound.
  Scan the real chapters for this pattern before committing to "one button
  per fence" as the only interaction.
