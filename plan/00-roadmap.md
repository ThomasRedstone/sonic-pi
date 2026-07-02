# Streamlined Sonic — Roadmap

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

## Phased migration

Each phase ships and is self-validating against the current app.

0. **Quick win (independent of the rewrite)** — wire up cross-pane copy in the
   existing Qt app (`addUniversalCopyShortcuts` is commented out at
   `app/gui/mainwindow.cpp:645`). Delivers the copy feel immediately.
1. **Rust core behind the existing contract** — reimplement `app/api` in Rust as
   a drop-in speaking the same OSC/shm to Spider + SuperSonic, **keeping the Qt
   GUI on top**. The existing UI is the oracle: if Sonic Pi still works
   unchanged, the Rust core is correct. Proves the hardest bit (the shm ring).
2. **New GPUI frontend** — build against the proven Rust core; run side-by-side
   with Qt until parity.
3. **Retire Qt** — fold daemon supervision into the Rust core; drop the C++ GUI
   and the per-platform build scripts.

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
