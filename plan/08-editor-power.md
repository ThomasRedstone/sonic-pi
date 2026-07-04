# Phase 10 — Editor power

> Read [`00-roadmap.md`](./00-roadmap.md) first. Lowest-risk of the
> post-parity phases — steady accumulation on top of `gpui-component`,
> mostly upstream-adjacent rather than novel engineering. Good filler work
> between the higher-priority phases, or when daily use surfaces a specific
> editor friction worth jumping the queue for.

## Goal

Close the remaining gap between "gpui-component's code editor" and "a code
editor a live coder reaches for without friction." Some of this is already
done and just needed enabling (see the `03-status.md` gotcha: folding and
Ruby syntax highlighting both existed behind unset flags/features and were
silently off) — audit before building.

## Candidates

### Multi-caret editing
Check `gpui-component`'s `InputState` for existing multi-cursor support
before assuming it needs building — the crate is described as
"~200K-line stable" in the roadmap's original evaluation, multi-caret is a
common enough feature it may already exist behind an unbound keybinding
(the same pattern as folding/highlighting). Audit first, bind second, only
build as a last resort.

### Keyboard fold toggle
Folding renders (chevrons in the gutter, mouse-clickable) but has no
keybinding — mid-performance, reaching for the mouse to fold a
`live_loop` breaks flow. Check `gpui-component`'s `input` module for a
`ToggleFold`-shaped action (the crate ships `Undo`/`Redo`/`Search` etc. as
bindable `actions!` — folding likely has a sibling); if present, add it to
the spike's `cx.bind_keys([...])` list in `main.rs` exactly like the
existing `ctrl-shift-z` redo alias. If genuinely absent, this is a small
upstream contribution (the fold state already lives in `display_map`,
per `update_fold_candidates` in `gpui-component`'s `state.rs`).

### Hover docs
The vocabulary (`vocab.rs`'s `Vocab`, already powering completion and the
Help pane) has everything needed for hover tooltips: hovering `:tb303` or
`cutoff:` in the editor could show the same `OptDoc`/summary the Help pane
renders, without leaving the buffer. Needs：
- A hover-detection hook on the editor (`gpui-component` likely has a
  hover/tooltip mechanism already, given it lists LSP support in its
  feature set — LSP hover is exactly this).
- Reuse `VocabEntry::doc_markdown()` (already built for the Help pane) as
  the tooltip content — no new formatting logic needed.

### Inline diagnostic text
Errors currently show as a red underline via `Diagnostic` (see the
`ClientEvent::Report` handling in `main.rs`'s tick loop) but the message
text only appears in the Log pane — a performer's eyes are on the code, not
the log, when something breaks. Check whether `gpui-component`'s
`Diagnostic` type supports a hover-to-reveal or always-visible inline
message (common in editors — e.g. a squiggle + a dimmed inline note at
end-of-line); if so, wire the existing error text through instead of just
the underline.

## Work items

1. **Audit pass first, before any new code**: grep `gpui-component`'s
   `input` module for multi-caret, fold-toggle actions, hover/tooltip
   support, and diagnostic rendering options. Given two features already
   turned out to be "just enable it," budget time for this audit — it may
   collapse most of this phase into flag-flipping.
2. For anything genuinely missing: scope as either a keybinding + a few
   lines of plumbing (cheap, do it), or an upstream `gpui-component` patch
   on the existing fork (`ThomasRedstone/gpui-component` — already set up
   for exactly this from the tutorial-images work; same PR-upstream
   discipline applies: patch, use, PR back, drop the pin once merged).
3. i18n + tests as usual for anything user-visible.

## Audit results (2026-07-04)

Did the audit-first pass the plan calls for, before writing anything new:

- **Hover docs**: ✅ **built.** `gpui-component` ships a `HoverProvider`
  trait mirroring `CompletionProvider` almost exactly (same shape as the
  existing `SonicCompletions` adapter) — no upstream work needed, just a
  second small adapter (`SonicHover`) over `Vocab`. Hovering a synth, fx,
  sample, function, or opt name now shows the same markdown doc the Help
  pane renders, reusing `VocabEntry::doc_markdown()` — no new formatting
  logic. The word-detection half needed a genuinely new pure helper
  (`vocab::word_at`) since completion's `word_prefix_at` only looks
  backward from the caret (what's typed so far) where hover needs the
  whole word under an arbitrary cursor position, forward and back — unit
  tested, plus the whole lookup (`hover_markdown_for`) factored out
  GPUI-free and unit tested directly (no `Window`/`App` needed).
- **Keyboard fold toggle**: ❌ **confirmed upstream-blocked, not a flag.**
  Unlike syntax highlighting and folding-the-feature (both were one config
  flag away), the actual fold/unfold mutation
  (`DisplayMap::toggle_fold(line)`) lives behind a `pub(super)` field —
  invisible outside `gpui_component::input` itself. There is no public
  method, action, or keybinding hook to reach it from application code.
  Confirmed by grep, not assumption. A real fix needs an upstream PR
  adding a public `InputState::toggle_fold_at_cursor()` or similar; not
  attempted here (a mouse-click-simulation workaround would be fragile and
  wasn't worth the risk for a "nice to have").
- **Multi-caret**: ❌ **confirmed absent, not hidden.** No trace of
  multi-cursor/add-cursor/select-next-occurrence anywhere in the crate.
  Not a quick win under any framing — would be substantial upstream work
  neither this session nor a patch-and-use-locally approach is well suited
  to. Left for whoever eventually drives the `gpui-component` fork harder
  (see also: PR-ing the tutorial-images `local-image-paths` patch, still
  pending).
- **Inline diagnostic reveal-on-hover**: 🔶 **viable path found, deferred.**
  Diagnostics (the red-underline `Diagnostic` type) don't automatically
  feed the hover popover — but `SonicHover` already has everything needed
  to ALSO check for a diagnostic at the hovered offset and show its
  message. The catch: `HoverProvider::hover` gets `(text, offset)`, not a
  reference to the specific `InputState` instance's live diagnostic set —
  reaching it would mean mirroring diagnostic state into a second,
  `Rc<RefCell<…>>`-shared place SonicHover can see, kept in sync with
  every place that pushes/clears diagnostics (`run_active`'s clear, the
  tick loop's push on error). Real, buildable, but real added state and a
  real drift risk — scoped as its own follow-up rather than rushed in
  alongside the vocab-hover work.

## Exit criteria

No fixed checklist — each candidate ships independently when ready. Track
progress as a running list in `03-status.md` the same way other phases
have been logged, rather than gating this file on all four candidates
landing together.

## Risks

- **Assuming absence without checking.** The folding/highlighting gotcha
  is the cautionary tale for this entire phase: don't write new code for
  something that's one feature flag or one keybinding away from already
  working. Audit is not optional busywork here — it's the highest-leverage
  step.
