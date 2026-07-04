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
