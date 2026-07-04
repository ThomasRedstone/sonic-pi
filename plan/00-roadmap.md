# Sonic Oxide — Roadmap

> Status: exploratory / pre-decision. Captures the direction agreed so far.
> Baseline reviewed: Sonic Pi `5.0.0-beta4` (this repo, `dev` branch).

## Vision

A leaner, faster, more editor-like Sonic Pi. **Keep the parts that *are* Sonic
Pi**; replace the shell around them.

- **Keep**: the music language (Ruby "Spider" runtime), the audio engine
  (SuperSonic, which wraps the SuperCollider synthesis core), and MIDI (already
  handled inside SuperSonic over OSC).
- **Replace**: the C++ Qt GUI, the C++ `app/api` bridge, and the Ruby
  `daemon.rb` supervision — collapsing them into a **single native Rust app**.

Primary goals driving this:
1. **Editor/UX feel** — trivial select & copy across every pane; a real
   editor, not clunky per-widget text areas.
2. **Dev velocity** — escape the 6,175-line `mainwindow.cpp` god object and the
   46-script per-platform build.
3. **Rust core** — the small, contract-defined C++ API layer is ripe for a
   Rust rewrite.

## Why this is tractable

Everything already talks through one seam: the `IAPIClient` interface + OSC
(UDP/TCP) + a shared-memory ring for scope/metrics. The GUI is genuinely a
*replaceable client*. So the frontend and bridge can be rebuilt incrementally
with the current app as a live reference, without touching audio or language
semantics.

## Current architecture (baseline)

```
Qt GUI (C++, ~27K LOC)  ──OSC/UDP + shm──▶  Daemon (Ruby)  ──spawns──▶  Spider runtime (Ruby, ~44K LOC)
        │  app/api C++ bridge (~3K LOC)          │                          └─ the music language
        └────────────────────────────────────────┴──spawns──▶  SuperSonic (audio engine; SC core + MIDI)
```

Notable: the old Erlang "Tau" scheduler is **already gone** — SuperSonic
replaced it. Bundled distribution also ships a full Ruby + ~20 gems.

## Target architecture

```
┌──────────────────────────────────────────────┐
│  Single Rust app                               │
│                                                │
│  Frontend (GPUI)  ◀── in-process ──▶  Rust core│
│  · code editor                        · OSC + shm ring
│  · docked panes (scope/log/cues/help) · FFT / scope
│  · panes = native elements            · process supervise
│    (select/copy free)                 · port alloc, keep-alive, kill-switch
└───────────────┬───────────────────┬───────────┘
        spawn+OSC │                   │ spawn+OSC
                  ▼                   ▼
        Ruby Spider (kept)     SuperSonic (kept)
        the music language     SC synthesis + MIDI
```

## Component decisions

### Frontend — GPUI (DECIDED)

> **Decision (locked):** native **GPUI** is the frontend. The accessibility
> sizing spike (see `01-spike-results.md`) showed Tier-1 (content readable) is
> cheap and Tier-2 (editable-text: caret/selection/nav/editing) is a bounded but
> real workstream we accept and own. Native speed + a single Rust binary won.
> The web/Tauri path below is retained only as a recorded fallback.



- **A Zed *extension* cannot do this.** Zed's WASM extension API only provides
  languages/grammars, themes, LSP, debuggers, snippets, MCP servers — **no
  custom UI/panels/visualizers**. Ruled out for the instrument itself.
- **Forking Zed** — rejected: inheriting a huge GPL, fast-moving upstream is a
  maintenance sinkhole for a small team.
- **GPUI custom app** — chosen direction. GPU-accelerated, pure Rust, standalone
  (`create-gpui-app`). Native editing speed sidesteps the "web is slow"
  concern and collapses frontend + core into one Rust binary (zero IPC hop).
  - Editor + panes from [`longbridge/gpui-component`](https://github.com/longbridge/gpui-component)
    (Apache-2.0): a high-perf code editor (Tree-sitter highlighting, LSP,
    ~200K-line stable) + a dock layout system + ~60 widgets.
- **Web/Tauri + CodeMirror 6** — pragmatic fallback if GPUI's immaturity or
  accessibility story blocks us.

### Rust core — replaces `app/api` (+ `daemon.rb` supervision)

`app/api` is ~3K LOC and its vendored deps map almost 1:1 to Rust crates:

| Current (C++/vendored)            | Rust replacement                    |
|-----------------------------------|-------------------------------------|
| oscpkt / kissnet OSC + UDP/TCP    | `rosc` + `tokio` (or std UDP)       |
| KissFFT (scope/spectrum)          | `rustfft`                           |
| reproc (spawn scsynth/spider)     | `tokio::process` / `std::process`   |
| PlatformFolders                   | `dirs` / `directories`              |
| ghc::filesystem, TLSF             | `std::fs`, `std::alloc`             |
| libsndfile (WAV I/O)              | `hound` / `symphonia`               |
| shared-memory ring (`server_shm.hpp`) | `raw_sync` + bespoke `#[repr(C)]` ring |

The hardest part is the **realtime shm ring** — must match SuperSonic's memory
layout exactly (`#[repr(C)]`, same atomics/ordering). Everything else is routine.
Folding `daemon.rb`'s supervision (kill-switch, keep-alive, port alloc) into the
Rust core drops one Ruby process (4 processes → 3).

## Phased migration — status as of 2026-07-03

Each phase ships and is self-validating against the current app.

0. ✅ **Quick win** — Log/Cues "Copy All" landed in the Qt app,
   runtime-verified.
1. ✅ **Rust core behind the existing contract** — `sonicpi-core` speaks the
   full OSC/shm contract (95% line coverage, golden-traffic regression
   fixtures from real sessions). The planned C-ABI shim was consciously
   dropped: validation happens via golden fixtures + side-by-side use
   instead of relinking the Qt GUI.
2. ✅ **New GPUI frontend (Sonic Oxide)** — the live-coding loop is at ~90%
   parity (plan/03-status.md has the exact gap list), running side-by-side
   with Qt. Editor a11y (AT-SPI Text interface) exceeds Qt.
3. ◕ **Consolidate** — daemon supervision folded into the core (3 processes,
   kernel-guaranteed cleanup, supervisor is the default boot);
   self-contained Linux packaging (71MB AppImage, CI artifacts). REMAINING:
   loop-to-100% + product gaps (03-status has the ordered plan),
   mac/windows, and the Qt retirement gate (sustained side-by-side use).
4. **Retire Qt** — drop the C++ GUI, app/api and the per-platform build
   scripts once the gate opens.

## Open risks / things not to lose

- **Accessibility** — Sonic Pi has real a11y commitments (schools, blind users;
  `accessibleName` usage, `mac-selftest-accessibility.sh`). GPUI's screen-reader
  story is **unproven and must be validated early** — potential dealbreaker.
- **GPUI maturity** — pre-1.0, frequent breaking changes; two moving git deps
  (GPUI + gpui-component). Budget for churn.
- **Editor customizations** — run-flash, live-coding cursor semantics, and the
  completion popup with piano/slider helpers (`completionpopup.cpp`, 1.5K LOC)
  are custom work on top of any editor component.
- **i18n** — translations are gettext across Ruby + Qt; a new frontend needs its
  own translation pipeline.
- **Non-issues (reassuring)** — MIDI lives in SuperSonic (untouched); Spider and
  SuperSonic are kept, so music-making semantics don't move.

## Immediate next step

Spike a GPUI app (`experiments/gpui-spike/`) with the gpui-component editor +
docked panes, then **test a screen reader against it** (the make-or-break
unknown) and prototype the scope by feeding the shm ring into a GPUI canvas. If
both pass, proceed with the phased plan above.

## Post-parity phases (2026-07-04 — proposed, not yet started)

Parity with Qt (loop + product) and the foundation workstream (conformance
harness, cross-platform core, editor a11y glyph metrics) are done — see
`03-status.md`. Retiring Qt (phase 4 above) is gated on *sustained daily use*,
which takes calendar time, not engineering time. These five phases are what to
spend that time on — they ask what Sonic Oxide should become, not just what
Sonic Pi already was. Each has its own numbered plan file; this table is the
map.

| Phase | Ships | Why it's worth doing | Plan |
|------:|-------|-----------------------|------|
| **6** | ✅ Gig-hardening — the runtime survives a GUI crash/relaunch; fuzzed OSC parsing; soak-tested | The one thing Qt never had; the most differentiated thing Oxide can be | [`04-gig-hardening.md`](./04-gig-hardening.md) |
| **7** | Teaching mode — runnable code blocks in the tutorial, first-run flow | Sonic Pi's identity is learn-by-sound; the tutorial renders but doesn't teach yet | [`05-teaching-mode.md`](./05-teaching-mode.md) |
| **8** | Performance UI — full-screen mode, projector-scale type, MIDI-controller-mappable actions | Only worth building once daily use says what's actually reached for | [`06-performance-ui.md`](./06-performance-ui.md) |
| **9** | Platforms & release — mac/win apps, README + versioned releases, the "go public?" decision | Hardware-gated for the app half; the release half is a product decision | [`07-platforms-release.md`](./07-platforms-release.md) |
| **10** | Editor power — multi-caret, keyboard folding, hover docs, inline diagnostics | Steady accumulation; mostly upstream-adjacent, low risk | [`08-editor-power.md`](./08-editor-power.md) |

**Recommended order:** 6 → 7, run in parallel with the daily-use clock; 9's
platform half starts whenever hardware is available; 8 and 10 are driven by
what daily use actually surfaces, not a fixed schedule.

**Deliberately not a phase:** replacing Spider with a Rust-native music
language. User code is Ruby: this DSL, this interpreter — a "rewrite" would
mean embedding an interpreter for it, reopening the settled keep-Spider
decision, and duplicating what upstream Sonic Pi actively maintains. The
measured baseline (`03-status.md`: boot ~2s, run-ack <5ms) shows this isn't a
performance problem looking for a solution.
