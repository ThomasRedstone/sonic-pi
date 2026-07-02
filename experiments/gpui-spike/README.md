# GPUI frontend spike

Throwaway spike for the "streamlined Sonic" direction — see
[`../../plan/00-roadmap.md`](../../plan/00-roadmap.md). It stands up a native
GPUI window with a real code editor and three split panes (Scope / Cues / Log)
to test the two make-or-break unknowns of the GPUI path.

## Build & run

```sh
cd experiments/gpui-spike
cargo run
```

### Live Scope from shared memory

The Scope can render live audio read from a shared-memory ring that mirrors
SuperSonic's real protocol. That reader now lives in the Rust core
(`sonicpi-core::audio::shm`, consumed here via a path dependency). Run the
stand-in writer, then the GUI:

```sh
cargo run --bin shm_writer      # terminal 1: "fake SuperSonic" streams audio
cargo run                       # terminal 2: GUI attaches, Scope shows live data
```

With the writer running, the Scope pane title reads **"Scope · live (shm)"** and
shows a moving waveform pulled from the ring; without it, the title reads
**"Scope · demo"** and a synthetic trace is drawn. Ctrl-C the writer to unlink
the segment.

Correctness of the ring (layout, atomics, wrap-past-capacity) is covered by
headless tests in the core: `cd ../rust-core && cargo test`.

⚠️ First build is heavy: it clones the Zed git repo and compiles GPUI + the
gpui-component library (hundreds of crates, several minutes, ~GBs). Linux needs
Vulkan + `libxkbcommon` + Wayland/X11 dev libs (already present on this box).

The `gpui` / `gpui-component` git revs are pinned in `Cargo.toml`. They are
pre-1.0 and drift often; if a build breaks, re-sync both revs against
gpui-component's own `Cargo.toml`.

## Current features (Phase 3a)

- **3 editor buffers** with a tab row; **Run ▶** sends the active buffer through
  `sonicpi-core` as `/save-and-run-buffer`; **Stop ■** sends `/stop-all-jobs`.
- **Real daemon boot with fallback**: startup tries `ruby daemon.rb` (handshake →
  `Session`); if the runtime is incomplete (no built SuperSonic) it falls back to
  an in-process loopback spider — the Log pane says which mode you're in. To get
  the real thing: `git submodule update --init` and build `app/external/supersonic`.
- **Run-flash** on the editor border on eval.
- **Live Log pane** rendering decoded engine events (`ClientEvent`s).
- **Accessibility**: panes have AccessKit roles/labels (Tier-1); the editor adds
  an `EditorA11y` node (role ENTRY) carrying the buffer text as its value —
  Tier-2 (full Text interface: caret/selection/text runs) is tracked work.

## What to evaluate

1. **Accessibility (the big one).** Run a screen reader (Orca on Linux) against
   the window. Can it announce the editor and the Cues/Log panes and follow the
   caret? Sonic Pi has real a11y commitments; if GPUI can't do this well, it
   sinks the whole approach — test this *first*.
2. **Select & copy.** Select text in the editor and in the Cues/Log panes and
   copy it. It should feel native and trivial (the original motivation). Every
   pane is a real text element, so cross-pane copy is just OS clipboard.
3. **Editor feel.** Typing latency, scrolling, line numbers, resizing the split.
4. **Scope canvas.** The Scope pane is custom-drawn (`canvas` + `paint_quad`)
   from a per-frame sample buffer — the stand-in for reading SuperSonic's
   shared-memory ring. "Run/Stop scope" toggles the animation.

## What this deliberately does NOT do

- Talk to Spider or SuperSonic over OSC (the Scope shm ring is real, but it
  reads a stand-in `shm_writer`, not the actual engine; no OSC control path yet).
- Ruby syntax highlighting is best-effort — depends on a bundled grammar; falls
  back to plain text otherwise.
- Use gpui-component's full `Dock`/`Panel` system (drag-out tabs, persistence).
  This spike uses simple `resizable` splits; the real dock is a later step.
- Reimplement live-coding editor behaviours (run-flash, completion popup, etc.).
