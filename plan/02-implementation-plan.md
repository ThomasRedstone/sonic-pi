# Sonic Oxide — implementation plan

> Detailed, phased plan for the rebuild. Read [`00-roadmap.md`](./00-roadmap.md)
> first for the vision and architecture; [`01-spike-results.md`](./01-spike-results.md)
> for what's already proven.
>
> **Guiding rule:** every phase ships something useful and leaves the app fully
> working. No big-bang cutover. The current Qt app stays the reference (and the
> fallback) until the new frontend has surpassed it.

## The invariant that makes this safe

Everything crosses one seam: **`IAPIClient` + OSC + a shared-memory ring**. As
long as we preserve that contract, we can replace what's on either side of it
one piece at a time, using the untouched other side as an oracle. We never move
the two things that *are* Sonic Pi — the Ruby "Spider" language runtime and the
SuperSonic audio engine (which also owns MIDI). Those stay put through every
phase.

## Phase map

| Phase | Ships | User-visible value | Biggest risk retired |
|------:|-------|--------------------|----------------------|
| **0** | Editor-feel fixes in the current Qt app | Trivial cross-pane copy, nicer editing — *today* | none (pure win) |
| **1** | Validation spikes (a11y + shm ring) | none (internal) | GPUI accessibility; shm layout |
| **2** | Rust core behind the existing contract, Qt on top | Faster boot, robust process cleanup, stable API | Rust core correctness |
| **3** | New GPUI frontend, opt-in, to parity | A fast, native, editor-first Sonic Pi | Frontend parity + feel |
| **4** | GPUI default; Qt + C++ + daemon.rb retired | Smaller, simpler, single-binary app | Build/packaging simplification |
| **5** | Distribution slimming (optional) | Smaller download, faster startup | runtime footprint |

Phases 0 and 1 can run in parallel. 2 and 3 overlap once 1 gives the green light.

---

## Phase 0 — Quick wins in the current app  *(first change landed)*

**Goal:** deliver the "editor-like feel" motivation immediately, with zero
architectural risk, while the bigger work spins up.

**What the review actually found:** copy is in better shape than the roadmap
assumed. The Log/Cues panes (`SonicPiLog`, a `QPlainTextEdit`) are already
mouse+keyboard selectable and already have a right-click Copy / Select All /
Clear menu; read-only `QPlainTextEdit` handles Ctrl+C/Ctrl+A natively when
focused. The commented-out `addUniversalCopyShortcuts(outputPane)` at
`mainwindow.cpp` was **correctly** left off: adding an explicit Ctrl+C `QShortcut`
to a widget that already copies natively triggers Qt's "ambiguous shortcut
overload" — the real cause of the old "steals events from doc system" note. So
re-enabling it would be a regression, not a fix.

**Done**
- Added a one-click **"Copy All"** to the Log/Cues context menu
  (`sonicpilog.cpp`) — copies the whole pane via the clipboard without disturbing
  the current selection (the common "paste a run's output into a bug report"
  action). Disabled when the pane is empty.
- Replaced the misleading dead code/comment in `mainwindow.cpp` with an
  explanation of why explicit copy shortcuts are intentionally omitted for these
  panes (ambiguity) and where copy actually comes from.

**Still open (cheap, optional)**
- Sweep for other editor-feel gaps (e.g. Copy All on the debug/OSC log panel).

**Value shipped:** grabbing an entire log/cue pane is now one click; the copy
story across panes is consistent and documented.

**Exit criteria:** copy works from every text pane via keyboard + context menu;
no regression in editor behaviour. *(Pending a full Qt build to runtime-verify —
no build dir present in this environment.)*

**Risk:** negligible (additive context-menu action; no shortcut/focus changes).

---

## Phase 1 — Validate the GPUI direction (decision gate)

**Goal:** turn the two remaining unknowns into evidence before committing real
build effort. Extends the existing spike; ships nothing to users.

**Work**
- **Accessibility test (the gate). ⚠️ done — mixed result.** Tested via the
  AT-SPI tree the app publishes (what Orca reads), not by having Orca speak. See
  `01-spike-results.md` for the full verdict. Summary: AccessKit activates and
  the app/window register, but **`gpui-component` exposes no accessible content**
  (0 AccessKit usage → editor/panes/buttons absent from the tree). GPUI core is
  a11y-capable; the widget library is not. So the stack is **not screen-reader-
  usable as-is**. This is the open decision (see below); it does not block the
  Rust core (Phase 2) or the shm work, which are frontend-agnostic.
  - **Sizing spike done** (see `01-spike-results.md`): instrumenting panes with
    `id`+`role`+`aria_label` (live text) made all four panes readable in the
    AT-SPI tree with correct roles — **Tier 1 (content readable) is small/proven**.
    **Tier 2 (caret/selection/text-nav/editing) is the real workstream**: the
    nodes lack the `Text`/`EditableText` interface; providing it means populating
    `accesskit::Node` text fields from `InputState`, upstream in gpui-component or
    a custom element. Bounded but non-trivial and largely ours to own.
  - **DECIDED: (A) commit to GPUI** and own the a11y workstream. Tier-2
    editable-text a11y (accesskit value/selection/text-runs from `InputState`) is
    now a first-class Phase-3 deliverable, applied per widget (editor first, then
    completion popup, dialogs), contributed upstream to gpui-component where we
    can. Accessibility is a per-sub-phase acceptance criterion, re-tested with the
    AT-SPI walk. The web/Tauri path is retired to "recorded fallback".
- **Shm ring spike. ✅ done.** A Rust reader/writer (`experiments/gpui-spike/src/shm.rs`)
  mirrors SuperSonic's real protocol byte-for-byte from the repo's C++ headers
  (`shm_audio_buffer.hpp` / `server_shm.hpp`): 32-byte slot header, interleaved
  float ring, `write_position` with Release/Acquire, self-describing segment
  header (MAGIC `0x5C09E006` → offsets → locate slot 0), real POSIX
  `shm_open`/`mmap`. A stand-in writer (`bin/shm_writer`) streams audio; the GUI
  Scope reads the latest window live (title flips to "Scope · live (shm)").
  Verified: compile-time layout asserts matching the C++ `static_assert`s;
  headless tests for roundtrip, latest-window, and wrap-past-capacity
  (`cargo test --lib`); cross-process run confirms the exact 1,536,384-byte
  segment and clean unlink on exit. Proves layout match + atomic ordering; the
  no-tear window read is documented (benign at real rates; the dedicated
  triple-buffered `shm_scope_buffer` path is tear-free by construction if needed).
- Editor feel gut-check vs QScintilla (latency, scrolling, large buffers).

**Value shipped:** a documented go/no-go with evidence, and the riskiest core
mechanism de-risked.

**Exit criteria:** Orca verdict recorded; scope rendering live data from a real
shm ring; frontend decision confirmed (GPUI or web fallback).

**Risks:** accessibility is the make-or-break — hence it gates here, cheaply,
before Phase 3's large investment.

---

## Phase 2 — Rust core behind the existing contract  *(scaffold landed)*

**Goal:** replace the C++ `app/api` layer (~3K LOC) with a Rust core, **keeping
the Qt GUI unchanged on top**. The existing UI + full feature set is the oracle:
if Sonic Pi behaves identically, the core is correct.

**Scaffold done** — `experiments/rust-core/` (standalone crate, stable Rust,
`cargo test` + `cargo run --example contract_demo`):
- `protocol` — the full OSC vocabulary + argument shapes lifted verbatim from
  `sonicpi_api.cpp` / `osc_handler.cpp`; outgoing builders and an incoming
  parser → `ClientEvent`. Tested (run-buffer wire round-trip, ack/multi parse).
- `ports` — daemon handshake parser (`daemon gui_listen gui_send scsynth
  tau_osc_cues token`). Tested.
- `osc` — localhost UDP sender + listening server (send/recv tested).
- `session` — wires the three senders + incoming server + keep-alive behind
  ports/token; exposes `run`/`stop`/`ping`/`shutdown`.
- `client` — Rust mirror of `IAPIClient`, collapsed to one `on_event(ClientEvent)`.
- `audio` — the Phase-1 shm scope reader is now **folded into the core**
  (`sonicpi-core::audio::shm`, tested; `AudioProcessor` wires it up; the GPUI
  spike consumes it via a path dep). TODO: `rustfft` spectrum + ballistics.
- Skeletons (need the runtime to exercise): `process` (spawn daemon + read
  handshake), `paths` (server-relative joins; user dirs TODO).

**Scope of the Rust core** (maps ~1:1 from C++ — see roadmap table): OSC server
+ client (`rosc`/`tokio`), process spawning (`tokio::process`), the audio
shared-memory ring + FFT for the scope (`rustfft`), port allocation, WAV I/O
(`hound`), platform paths (`directories`).

**Integration decision (recommended):** expose the Rust core with a **C ABI
shim matching the current `IAPIClient`/`SonicPiAPI` surface**, linked into the
existing Qt GUI as a drop-in replacement for the C++ lib. This keeps the process
model byte-for-byte identical, maximising the oracle's value. The shim is
throwaway glue discarded in Phase 4. (Alternative: run the core as a sidecar
process the GUI talks to over local OSC — closer to nothing in the end state, but
changes the process model during validation. Prefer the drop-in.)

**Work**
- Rust core crate implementing the contract, with unit + golden tests against
  captured OSC traffic from the current app.
- C-ABI shim; swap the GUI's link target behind a build flag so we can A/B.
- Fold in incidental wins the rewrite enables: cleaner error reporting, more
  robust zombie-process cleanup, faster/leaner boot.
- CI: build both configurations; run the api-tests suite against the Rust core.

**Value shipped:** even with no UI change, users get a more robust, better-tested
core — faster boot, fewer stray `scsynth`/`supersonic` zombies, clearer errors —
and we retire a chunk of C++.

**Exit criteria:** the Qt app on the Rust core passes the existing test suites
and manual parity checks; scope/metrics/logs/cues/run all behave identically;
shipped behind a flag, then defaulted on.

**Risks:** realtime shm correctness (already de-risked in Phase 1); OSC edge
cases (mitigated by golden tests from real traffic).

---

## Phase 3 — New GPUI frontend to parity

**Goal:** build the real native frontend against the Phase 2 Rust core, shipped
**opt-in** alongside Qt, growing in usable increments until it surpasses Qt.

Sequenced so each sub-phase is a usable Sonic Pi for someone:

- **3a — MVP live-coding.** Editor (multi-buffer tabs) + Log + Run/Stop, talking
  to the Rust core. *You can already make music from the new frontend.* Includes
  the non-negotiable editor behaviours: syntax highlight, auto-indent, run-flash
  on eval, error highlighting, comment/uncomment.
  - **Slice 1 landed** (frontend↔core integration): "Run ▶" sends the editor
    buffer via `sonicpi-core`'s real OSC/`protocol`; decoded engine events render
    live in the Log pane. Proven headlessly by `rust-core`'s `loopback_run`
    example.
  - **Slice 2 landed** (3a build-out):
    - **Real-daemon boot with fallback**: on startup the app spawns `daemon.rb`
      via `sonicpi-core::process::Daemon` (4s handshake budget) and connects a
      full `Session` (senders + incoming server + keep-alive); when the local
      runtime is incomplete it falls back to the in-process loopback spider and
      says so in the Log. Verified live: the AT-SPI walk showed the Log label
      "=> Real daemon unavailable (daemon handshake…)" — i.e. the attempt ran
      and degraded exactly as designed. `Daemon::boot` is integration-tested
      against a stub daemon (handshake parse + clean failure, `tests/daemon_boot.rs`).
    - **Multi-buffer editor**: 3 buffers with a tab row; Run sends the active
      buffer as `buffer<N>`.
    - **Stop ■**: `/stop-all-jobs` through the same backend (loopback acks).
    - **Run-flash**: editor border flashes on eval (live-coding affordance).
    - **Tier-2 a11y increment**: `EditorA11y`, a custom GPUI `Element` wrapping
      the editor, joins the a11y tree as role ENTRY and writes the buffer text
      into `accesskit::Node::value`. **Finding:** the AT-SPI node still exposes
      only `Accessible`+`Component` — AccessKit's Unix adapter does not surface
      a Text/Value interface from `value` alone; full Text support requires
      AccessKit text runs + character positions. Tier-2 is now precisely scoped.
  - **Slice 3 landed** (editor behaviours):
    - **Comment/uncomment**: `#` button toggles `# ` on the selected lines (or
      cursor line) — pure `toggle_comment()` helper, unit-tested (3 tests:
      block round-trip, cursor-line, indentation-preserving uncomment).
    - **Error highlighting**: incoming `/error`//`/syntax_error` events now carry
      through as structured `ClientEvent`s (the GUI queue holds events, not
      strings); reports with a line number push a `Diagnostic` (severity Error,
      full-line range) onto the active buffer — red underline in the editor.
      A fresh Run clears the previous run's diagnostics. Loopback spider
      simulates it: a line containing `boom` triggers `/syntax_error` at that
      line, so the path is demoable without the runtime.
  - **Local runtime: in progress.** `supersonic` submodule is initialised and
    its CMake build (self-contained FetchContent: JUCE, libsndfile, codecs —
    no vcpkg) gets to 73% but needs **`libjack-jackd2-dev`** (JUCE_JACK=1 is
    deliberate upstream: dlopens libjack at runtime, needs headers at compile
    time). All other JUCE Linux deps present. After install: rebuild, copy the
    binary to `app/server/native/supersonic`, and `Backend::try_real` should
    connect the GUI to the real daemon+Spider unchanged. Fun fact: the build
    exposes a `supersonic_rust` target — the engine already embeds Rust.
  - Remaining for 3a: auto-indent tuning, per-run log formatting, Tier-2
    text-runs a11y, and the real-runtime end-to-end run.
- **3b — Feedback panes.** Scope (canvas ← shm ring, from Phase 1), Cues,
  Metrics panel, Link metronome/BPM.
- **3c — Discoverability.** Autocomplete + the completion popup (docs, and the
  piano/slider helpers — `completionpopup.cpp` is 1.5K LOC, budget for it) and
  the Help/docs browser (renders the existing `doc.html`/`info.html`).
- **3d — Configuration.** Settings (audio/MIDI/display/keyboard — `settingswidget.cpp`
  is 2.4K LOC), MIDI device config, theme system, i18n/translation pipeline.
- **3e — The long tail.** Recording, node-tree graph, OSC/debug log panel,
  platform features (Syphon/Spout publish), remaining preferences.

**Parity checklist** (the honest scope — each must be reproduced or consciously
dropped):

| Area | Current (Qt) | Notes |
|------|--------------|-------|
| Multi-buffer editor | QScintilla, 10 tabs | tabs, per-buffer state |
| Live-coding behaviours | run-flash, auto-indent, cursor semantics | custom on top of any editor |
| Autocomplete + popup | `ScintillaAPI`, `completionpopup.cpp` (1.5K) | piano/slider helpers |
| Syntax highlighting | QScintilla lexer | need a Sonic Pi/Ruby grammar |
| Log / Cues | `SonicPiLog` (QPlainTextEdit) | native, selectable |
| Scope | OpenGL `ScopeWindow` (680) | → GPUI canvas + shm |
| Metrics panel | `metricspanel.cpp` (1.7K) | shm-fed live dashboard |
| Settings | `settingswidget.cpp` (2.4K) | large surface |
| Help/docs | QTextBrowser + doc html | reuse generated docs |
| Themes | `sonicpitheme.cpp` (1.3K) | map to gpui-component theme |
| i18n | gettext (Ruby + Qt) | new frontend needs its own path |
| Accessibility | `accessibleName` throughout | must not regress (Phase 1 gate) |
| Recording | platform `recorder_*` | Syphon/Spout, per-OS |

**Value shipped:** from 3a onward, early adopters get a fast, native,
editor-first Sonic Pi and drive feedback; every sub-phase is independently
usable.

**Exit criteria:** GPUI frontend reaches feature + accessibility parity with Qt
and is preferred by test users.

**Risks:** parity scope is the real cost of the project (see matrix); editor
behaviours and settings are the big rocks. Mitigation: ship incrementally, keep
Qt available, prioritise by usage.

---

## Phase 4 — Consolidate and retire Qt

**Goal:** make GPUI the default, collapse to a single Rust binary, and delete the
old shell.

**Work**
- Fold `daemon.rb`'s supervision (port allocation, keep-alive, zombie kill-
  switch) into the Rust core → **one fewer process** (4 → 3: Rust app, Spider,
  SuperSonic).
- Make GPUI the default frontend; remove the Qt GUI, the C++ `app/api`, and the
  Phase 2 C-ABI shim.
- Replace the ~46 per-platform build scripts with a unified Rust/Cargo build +
  a thin packaging layer per OS (bundle Ruby + SuperSonic + assets).
- Update CI/release: notarization (macOS), AppImage (Linux), installer (Windows)
  around the single binary.

**Value shipped:** the streamlined build realised — dramatically less C++, one
build system, one binary, fewer moving processes, faster startup.

**Exit criteria:** Qt/C++ removed; all platforms build + package from the unified
pipeline; release artifacts pass the existing platform smoke/accessibility tests.

**Risks:** packaging/signing per-OS is fiddly; do it behind the scenes while Qt
is still shippable, cut over only when green on all platforms.

---

## Phase 5 — Distribution slimming (optional / future)

**Goal:** attack the remaining footprint now that the shell is lean.

**Work (candidates, evaluate later)**
- Trim the bundled Ruby + ~20 vendored gems to what's actually used.
- Investigate a lighter/faster Ruby (or AOT/bootsnap-style) startup.
- Precompile/prune assets (samples, synthdefs, docs) by tier.

**Value shipped:** smaller download, faster cold start. Purely additive; do it
only if the footprint still warrants it after Phase 4.

---

## Cross-cutting concerns (apply to every phase)

- **Accessibility** is a first-class acceptance criterion, not a Phase 3 line
  item. Re-test with Orca/VoiceOver/Narrator at each frontend milestone. It
  gated the whole approach in Phase 1 for a reason.
- **The language never moves.** No phase touches the Ruby DSL semantics or the
  music model. MIDI stays inside SuperSonic. This is what keeps the risk bounded.
- **Testing:** golden OSC traffic captured from the current app is the safety net
  for the Rust core; the Qt app is the behavioural oracle until Phase 4.
- **Keep it shippable:** at every phase the app works and is releasable. If a
  phase stalls, we still have a better product than we started with.
- **Pinned pre-1.0 deps:** GPUI + gpui-component drift; bump deliberately and
  re-run the a11y test after each bump.

## Immediate next actions

1. Run the Orca accessibility test on the spike; record the verdict (Phase 1).
2. Spike the shm ring reader into the Scope canvas (Phase 1).
3. In parallel, land the Phase 0 cross-pane copy fix in the current Qt app.
