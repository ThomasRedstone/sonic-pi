# Spike results — GPUI frontend

> Outcome: **success.** A native GPUI window with a real code editor and docked
> Scope/Cues/Log panes builds and runs. The GPUI + gpui-component path is viable
> enough to commit to the next steps. One decisive test remains: accessibility.
>
> Spike lives at [`../experiments/gpui-spike/`](../experiments/gpui-spike/).
> Direction it serves: [`00-roadmap.md`](./00-roadmap.md).

## What we proved

- **GPUI runs as a standalone app** (no Zed, no fork) using
  [`longbridge/gpui-component`](https://github.com/longbridge/gpui-component) for
  the editor + widgets.
- **A real code editor** — gpui-component's `InputState`/`Input`, with line
  numbers, mono font, seeded with a Sonic Pi snippet.
- **Docked, resizable panes** — Editor | (Scope / Cues / Log), via `resizable`
  splits. Every pane is a genuine text element, so **select + copy is native**
  (the original motivation).
- **Custom GPU-drawn Scope** — a `canvas` + `paint_quad` waveform driven from a
  per-frame sample buffer, animated ~30fps. This is the exact shape of the
  eventual "read a frame from SuperSonic's shm ring" path — proving the drawing
  half now, before the data half exists.
- **Builds clean and launches clean** — `cargo build` exit 0; a 6s smoke-run
  stayed up with no panic or stderr.

## Encouraging signal on the big risk

During the build, GPUI pulled in and compiled `accesskit_atspi_common`,
`atspi`, and `accesskit_unix` — i.e. it wires into **AT-SPI**, the same
accessibility bus Orca uses. The plumbing exists; it is not merely aspirational.
This does **not** yet prove it's *usable* — that needs the Orca test below — but
it moves accessibility from "unknown if possible" to "present, needs validation."

## Gotchas hit (documented so we don't relearn them)

1. **Toolchain.** gpui (at the pinned rev) needs **Rust 1.95** — it uses
   `std::hint::cold_path()`, unstable before 1.95. Fixed with a
   `rust-toolchain.toml` pinning the 1.95 nightly. Upstream Zed pins stable
   `1.95.0`. Symptom on an old toolchain: `error[E0658]` in gpui's `profiler.rs`.
2. **System lib.** Linking needs `libxkbcommon-x11-dev` (the runtime `.so.0` is
   not enough — the dev symlink is required for `-lxkbcommon-x11`). The `x11`
   feature is forced transitively by gpui-component even on a Wayland session, so
   it can't be feature-flagged away. `sudo apt-get install libxkbcommon-x11-dev`.
3. **Pre-1.0 API drift.** Two small signature fixes were needed against the
   library's current `main` (`multi_line(true)` takes a bool; an unused import).
   Expect this on every dependency bump — the revs are pinned for exactly this
   reason.
4. **Debug binary is ~658 MB.** Expected for an unoptimized GPUI build with
   debuginfo; a release build is far smaller. Not a concern, just don't be
   alarmed.

## Accessibility verdict (AT-SPI test) — ⚠️ mixed, gates the decision

Rather than have Orca speak on a live desktop, we tested the thing Orca actually
consumes: the **AT-SPI accessibility tree** the app publishes on the a11y D-Bus.
Method: enable `org.a11y.Status` (IsEnabled + ScreenReaderEnabled), launch the
spike, walk its tree via `busctl` on `/run/user/1000/at-spi/bus`, restore flags.

Findings:
- ✅ **The foundation works.** The app registers on the a11y bus and AccessKit's
  AT-SPI adapter activates: it publishes an `application` node (role 75) with one
  child `frame`/window (role 23). GPUI core *has* an a11y API
  (`gpui/src/window/a11y.rs`, a11y on `div`/`text` elements).
- ❌ **The content is not exposed.** The window has **zero** child accessible
  objects — no editor, no panes, no buttons, no editable text, no caret. Only ~4
  accessible objects exist on the whole connection (desktop/app/frame shell).
- 🔎 **Root cause:** `gpui-component` has **zero** AccessKit usage (`grep
  accesskit` → 0 hits across the crate). Its widgets — including the code editor
  (`InputState`/`Input`), dock panes, and buttons — don't emit accessibility
  nodes. So a screen reader attaching today would find an app and an empty
  window and have nothing to read.

**Implication.** As-is, the GPUI + gpui-component stack is **not usable by a
screen reader** for the actual UI. This is not a GPUI-platform blocker (the
AccessKit plumbing + a11y API are present and working) — it's a **widget-library
maturity gap**. Committing to GPUI means owning real accessibility work:
instrumenting gpui-component's widgets (hardest: the editor's text/caret/
selection) via GPUI's a11y API, likely upstream. The web/Tauri + CodeMirror
fallback gets mature editor/DOM a11y largely for free.

Given Sonic Pi's a11y commitments, this is decision-affecting.

### Sizing spike — instrumented the panes, re-ran the walk

Added `id` + AccessKit `role` + `aria_label` (carrying live text from
`InputState::value()`/`cursor()`) to each pane. Re-ran the AT-SPI walk:

- Tree went from the empty shell (4 objects) to **8 objects**; the window now has
  **4 readable children**, each with the correct role and its content as the
  accessible name:
  - editor → role **79 = ENTRY**, name = "Sonic Pi code editor, 256 characters,
    cursor at offset 0. Content: # S…" (the actual code)
  - scope  → role **27 = IMAGE**, name = "Audio scope waveform"
  - cues   → role **39 = PANEL**, name = the cue text
  - log    → role **39 = PANEL**, name = the run-log text
- So a screen reader would now **announce the editor's code and the pane
  contents**. (The blank AT-SPI role *name* is an AccessKit-adapter quirk; the
  numeric roles are correct.)

**Effort estimate — two tiers:**

- **Tier 1 (content readable): small — proven here.** ~30 lines of `id/role/
  aria_label` wiring + feeding live text. Doing it across the real app is
  mechanical: wrap each widget, feed its text. Gets Orca reading everything as
  node *names*.
- **Tier 2 (true editable-text a11y): the real work.** Every pane exposes only
  `Accessible` + `Component` — **not** the `Text`/`EditableText` interface. So
  there is still **no caret tracking, no character/word/line navigation, no
  selection reporting, and no edit actions** — exactly what a code editor needs.
  Providing it means populating `accesskit::Node` value + text-selection + text
  runs from `InputState`'s rope/cursor/selection, via either (a) upstream a11y
  support in gpui-component's `Input`, or (b) a custom element/fork that writes
  those fields. AccessKit has the APIs and `InputState` exposes the data, so it's
  **bounded but non-trivial**, and — since gpui-component has *zero* a11y today —
  likely an ongoing upstream commitment (editor, completion popup, dialogs).

**Read:** GPUI accessibility is *achievable*, not free. Tier 1 is cheap; tier 2
is a real workstream we'd largely own. The web/Tauri + CodeMirror path gets tier
2 (caret, selection, text nav, editing) from the browser for free. Decision (A:
commit to GPUI + own the a11y work, vs B: web fallback) recorded with the team.

Secondary subjective checks (typing latency, scroll feel vs QScintilla) remain
worth doing interactively.

## Deliberate non-goals of the spike

No backend (no OSC, no shm, no Spider/SuperSonic); best-effort Ruby highlighting;
`resizable` splits rather than the full `Dock`/`Panel` system; none of the
live-coding editor behaviours (run-flash, completion popup, auto-indent).

## Update — shm ring read proven

The data half of the Scope is done. `experiments/gpui-spike/src/shm.rs` is a
byte-for-byte Rust mirror of SuperSonic's audio-buffer protocol (from this repo's
`shm_audio_buffer.hpp` / `server_shm.hpp`): real POSIX `shm_open`/`mmap`, the
32-byte slot header + interleaved float ring, `write_position` with
Release/Acquire, and the self-describing segment header (validate MAGIC → read
offsets → locate slot 0). A stand-in writer (`bin/shm_writer`) streams audio and
the GUI Scope renders the latest window live.

Evidence: compile-time layout asserts (the Rust twins of the C++
`static_assert`s); `cargo test --lib` covers roundtrip / latest-window /
wrap-past-capacity; a cross-process run produced the exact 1,536,384-byte segment
and unlinked cleanly on exit. This retires the "match the engine's memory layout
and atomics" risk — the hardest piece of the eventual Rust core.

## Verdict

Green to proceed. The remaining gate is the **Orca accessibility test** above.
Next core step per the plan: stand up the Rust core behind the existing OSC
contract. See [`02-implementation-plan.md`](./02-implementation-plan.md).

<!-- Screenshot: add a capture of the running spike here. -->
