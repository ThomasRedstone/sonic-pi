# Status & next priorities

> Session handoff — written 2026-07-02 so work can resume in a fresh context.
> Read order for a new session: this file, then `02-implementation-plan.md`
> for detail, `00-roadmap.md` for the why.

## Decisions (settled — don't re-litigate)

1. **Keep** the Ruby Spider runtime + SuperSonic engine (+ MIDI, inside
   SuperSonic). **Replace** Qt GUI + C++ `app/api` + `daemon.rb` supervision
   with one native Rust app.
2. **Frontend: GPUI** (native, gpui-component widgets). Web/Tauri is a recorded
   fallback only. We accept + own the Tier-2 editable-text a11y workstream.
3. Validation strategy: everything speaks the existing OSC/`IAPIClient`/shm
   contract, so the current app remains the oracle.

## What exists and works (all tested)

- **`experiments/rust-core/`** — the Phase-2 Rust core (stable Rust, no GPUI):
  OSC vocabulary + parsers (`protocol`), daemon handshake (`ports`), UDP
  transport (`osc`), session wiring w/ keep-alive (`session`), `IAPIClient`
  mirror (`client`), daemon spawn (`process`, integration-tested vs stub),
  **shm scope reader** (`audio::shm` — byte-for-byte mirror of SuperSonic's
  ring; layout/atomics/wrap tested). 9 unit + 2 integration tests green.
  Examples: `contract_demo`, `loopback_run`.
- **`experiments/gpui-spike/`** — the GPUI frontend (needs Rust 1.95 via its
  `rust-toolchain.toml`, plus `libxkbcommon-x11-dev`):
  3-buffer editor w/ tabs, Run ▶ / Stop ■ through the core's real OSC,
  run-flash, live Log pane (structured `ClientEvent`s), error-line red
  underline via `Diagnostic`, comment-toggle (`#`), Scope canvas reading the
  shm ring (`shm_writer` = fake engine), Cues pane.
  **Startup tries the real `daemon.rb` → `Session`; falls back to an
  in-process loopback spider and says so in the Log.**
- **Accessibility**: panes have AccessKit roles/labels (Tier-1); editor is in
  the a11y tree as ENTRY via a custom `EditorA11y` element carrying the text
  as value. **Verified via AT-SPI bus walk** (busctl on
  `/run/user/1000/at-spi/bus` with org.a11y.Status enabled — see
  `01-spike-results.md` for the method).
- **Qt Phase-0 fix** (uncommitted in working tree): "Copy All" on Log/Cues
  context menu (`app/gui/widgets/sonicpilog.cpp`) + comment fix in
  `mainwindow.cpp`. Needs a Qt build to runtime-verify.

## ✅ MILESTONE: the audible test PASSED (2026-07-02)

Human-verified: Run ▶ on buffer 1 against the real stack → drums heard
(first attempt was silent — audio was routed to headphones, not the
speakers; the stack was fine). Still to exercise by ear: Stop, buffer
switching, `#`, error underline against real Ruby errors.

## ✅ MILESTONE: real-runtime E2E ACHIEVED (2026-07-02)

SuperSonic built (after `libjack-jackd2-dev`; binary deployed to
`app/server/native/supersonic`, ~11.5MB Release). Launched the GPUI app:
**Log reads "=> Connected to REAL Sonic Pi daemon."** followed by live engine
traffic (`/supersonic/devices`, `/supersonic/input-devices`, `/supersonic/info`,
…). Daemon log confirms the full stack ran: SuperSonic JUCE audio device active
(hundreds of callbacks), MIDI ports enumerated, Spider booted. The GPUI
frontend → Rust core → real daemon/Spider/SuperSonic path works end-to-end.

Two follow-ups discovered:
- **Shutdown**: the spike doesn't send `/daemon/exit` on quit, and the daemon's
  keep-alive kill-switch took >60s to fire (had to pkill). Wire
  `session.shutdown()` into app quit (and consider Drop on `Backend::Real`).
- The engine floods `/supersonic/*` + other unmodelled messages on connect —
  they surface as `Unhandled` in the Log (correct behaviour, now needs
  modelling; see priority 4).

## Done 2026-07-02 later session (27 tests green: rust-core 18, spike 9)

- **Clean shutdown RUNTIME-VERIFIED**: `SONIC_SPIKE_AUTOQUIT=<secs>` env quits
  through the same path as window close (Wayland WM close can't be scripted);
  observed "daemon exited cleanly", exit 0, no leftover processes, shm
  unlinked. The golden-capture example verified the polite path a second time.
- **✅ TIER-2 A11Y ACHIEVED (the Phase-1 gate is fully closed).** `EditorA11y`
  now publishes AccessKit `TextRun` children (per line: value incl. `\n`,
  UTF-8 char lengths, word starts) + `text_selection` mapped from
  `InputState`'s byte offsets. **Verified via busctl walk: the editor node
  exposes `org.a11y.atspi.Text`** — CharacterCount/CaretOffset/GetText all
  answer with real buffer content; line-granularity `GetStringAtOffset` works.
  Known gaps: WORD granularity unsupported by AccessKit's AT-SPI adapter
  (upstream); run NodeIds are one frame stale (selection valid from frame 2);
  no `character_positions`/`widths` (screen-magnifier caret tracking) yet.
  Mechanism notes: child elements' a11y nodes nest via the prepaint stack;
  NodeIds mirror `GlobalElementId`'s DefaultHasher hash (`accesskit_id_of`).
- **Editor align**: `reindent()` (do/end-aware, 2-space, modifier-`if` safe) +
  "⇥ Align" button — the Qt "align text" equivalent. Enter already keeps
  indentation (gpui-component code-editor mode does that natively).
- **Log pane rebuilt**: structured `LogLine`s — per-run colour cycling (6-colour
  palette like `SonicPiLog`), errors red, multi-message rows indented under a
  `{run N}` header.
- **Cues pane LIVE**: `ClientEvent::Cue` (from `/incoming/osc`) → cues editor
  (kept selectable); first real cue replaces the placeholder; 200-line cap.
- **Protocol coverage**: `/supersonic/devices` (names-until-first-int wire
  quirk), `/supersonic/input-devices`, `/supersonic/info` now parse into
  `AudioDevices`/`AudioInputDevices`/`Scsynth` events, tested. `/version` was
  already modelled.
- **Metrics from shm**: `MetricsReader` (self-describing `PerformanceMetrics`
  u32 array; `metrics_idx` constants; `link_bpm()` decodes milli-BPM). Header
  shows live "N BPM · Link peers".
- **Golden OSC fixtures**: `examples/golden_capture.rs` boots the real daemon,
  drives a silent (`amp: 0`) run, records 163 real messages hex-encoded to
  `tests/fixtures/golden_osc.hex`; `tests/golden_osc.rs` asserts every address
  is recognised and ≥80% fully parse. Refresh: rerun the example.
- **UX from live user feedback**: colour-only Run/Stop press pulses (~250ms,
  no layout shift), ●playing/○idle indicator, Stop danger-red while playing,
  instant log echo on clicks. **Dark mode default** + ☀/☾ header toggle
  (`gpui_component::Theme::change`).
- **Real scope confirmed live end-to-end** (user-visible waveform from the
  engine's triple-buffered scope slot 0).
- Gotcha: SIGTERM-killed engines leave stale `/dev/shm/SuperSonic_*` segments
  (no unlink); remove stale ones before probing. Clean quits unlink correctly.

## Done 2026-07-02 evening session (all committed; 23 core + 16 spike tests)

- **3c autocomplete**: `CompletionProvider` over a repo-loaded vocabulary
  (synth/fx cheatsheets, samples dir, 239 lang `doc name:`/`summary:` blocks);
  `:sym` prefix restricts to symbols. Integration test loads the real repo
  files (>300 entries).
- **Keyboard shortcuts** via GPUI actions: Alt+R run, Alt+S stop, Alt+/
  comment, Alt+M align, Alt+[/] buffer prev/next.
- **10 buffers + workspace persistence**: `~/.sonic-pi/store/streamlined/
  buffer_N.spi`, load-on-boot / autosave 5s / save-on-quit. E2E-verified
  (marker survives a full boot→quit lifecycle; AUTOQUIT run writes all 10).
- **3d settings pane** (⚙): output/input device pickers from the engine's
  device pushes; switching sends `/daemon/audio/switch-device` (wire format
  mirrored from `MainWindow::sendDeviceSwitch`, unit-tested).
- **3b**: FFT **spectrum analyser** scope mode (rust-core `SpectrumProcessor`,
  exact C++ AudioProcessor constants/ballistics, rustfft; sine-bucket/silence/
  ballistics tests) + **Link tempo controls** (−5/+5, tap tempo →
  `/clock/tempo/set`) + live engine metrics strip.
- **3c Help pane** (?): ranked substring search over the vocabulary; entry
  view shows summary + synth/fx opts from the cheatsheets.
- Env gotcha: `pkill -f <pattern>` matches the harness's own bash wrapper
  (the command text embeds the pattern) and kills the shell — use
  `pkill -x sonic-gpui-spik` (comm name, 15-char truncated).

## Done 2026-07-02 (continued)

- **Opt completion (3c)**: synth/fx opts (`cutoff:`, `mix:`, defaults from
  the cheatsheets) complete after their symbol — last symbol before the
  caret wins, spanning lines (use_synth-persists semantics).
- **Recording (3e)**: `/supersonic/record/start|stop` (JUCE-side recorder,
  the DiskOut-free path Studio#recording_start uses). ⏺ Rec button →
  `~/.sonic-pi/store/sonic-oxide/recordings/`. **E2E PASS**:
  `cargo run --example record_check` boots the runtime, records 5s,
  validates the RIFF/WAVE on disk (1.4MB), exits non-zero for CI.
- **Qt app BUILT and RUNNING** (Phase-0 closeout): needed the missing dev
  packages + `./linux-pre-translations.sh` first (generates
  `gui/utils/ruby_help.h` — cmake fails without it). Binary:
  `app/build/gui/sonic-pi`. Running side-by-side with Oxide (the Phase-3
  oracle setup). Copy All context-menu smoke test: awaiting the human
  right-click.

## Done 2026-07-02 (late session)

- **Node-tree pane** (♪): `NodeTreeReader` mirrors NodeTreeHeader/NodeEntry
  from shared_memory.h; version-gated refresh; synthetic-segment test.
- **Debug-log toggle** (Dbg): internals rows (unmodelled OSC etc.) tagged
  and hidden by default — Qt hide/reveal-debug-logs parity.
- **Fixes from live use**: pane bodies clip + min_w(0) (device list bled
  across panes; long lines pushed the layout wider than the window, moving
  the titlebar controls off the corner — user-confirmed fixed); /incoming/
  osc parsed in the wrong arg order (address got the timestamp — regression-
  tested); cue rows now `+N.NNNs addr args` relative to the first cue;
  client-side window controls (– □ ✕) + title drag (no SSD on GNOME
  Wayland); close routes through clean shutdown.
- **CI**: `.github/workflows/sonic-oxide.yml` — both suites on push/PR.
- **A11y re-verified** post-changes via AT-SPI walk (all rendered panes
  expose role+name; editor Text interface unchanged). NOTE: the AccessKit
  adapter registers on `ScreenReaderEnabled`, dynamically — no app restart
  needed; `IsEnabled` alone does not trigger registration. WARNING: setting
  ScreenReaderEnabled starts Orca SPEAKING on the user's desktop — keep the
  window short and kill it with `pkill -f bin/orca` (it runs as
  `python3 /usr/bin/orca`, so `pkill -f "^orca"` misses).
- 24 core + 18 spike tests green.

## ✅ MILESTONE: Phase-4 Rust supervisor WORKS (2026-07-02)

`sonicpi-core::supervisor` boots Spider + SuperSonic directly — no
daemon.rb, 4 processes → 3. Port allocation via OS (pairs collapsed as the
daemon's table does), SuperSonic readiness = its shm segment appearing,
Spider spawned with SpiderBooter's exact argv (`spider-server.rb`, NOT
sonic-pi-server.rb). Crash-safety: PR_SET_PDEATHSIG(SIGTERM) on children —
kernel reaps them if the app dies, no 40s+ kill-switch window.
E2E `supervisor_check`: 73 events over a real Session, teardown to zero —
PASS. Spike opts in with `SONIC_OXIDE_SUPERVISOR=1`; daemon.rb remains the
default + A/B oracle. NOT yet ported: TOML audio opts, device switching
(needs an engine-restart path), the osc-cues external listener (daemon.rb
binds 4560; supervisor allocates dynamically — external OSC/MIDI cue
senders expect 4560). One unexplained orphaned-children incident during
bring-up → `shutdown_verified()` + WARNING log; watch for recurrence.

## Done 2026-07-02 (supervisor gaps + dock-lite)

- Supervisor: osc-cues prefers well-known **4560** (external cue senders),
  dynamic fallback; **audio-settings.toml applies** (flat-TOML → SuperSonic
  CLI flags per OPTS_TOML_KEY_CONVERSION). Both unit-tested;
  supervisor_check e2e re-run PASS. Remaining gap: device switching (needs
  an engine-restart path) — daemon.rb stays the spike default until then.
- **Collapsible panes** (dock-lite): click any pane title to fold it.
- 26 core + 18 spike tests green.

## ✅ MILESTONE: supervisor is the DEFAULT boot (2026-07-02)

Device switching turned out to need no engine restart — daemon.rb only
ever FORWARDED `/daemon/audio/switch-device` → `/supersonic/devices/switch`
(JUCE hot-swaps in place). Session now sends the switch direct in
supervisor mode, and registers itself for device pushes via
`/supersonic/devices/report [gui_listen]` on connect (both modes).
supervisor_check asserts daemon-free device pushes (3 received) — PASS.
Boot chain: supervisor default → daemon.rb fallback (`SONIC_OXIDE_DAEMON=1`
forces it) → loopback. Sonic Oxide now runs 3 processes by default.

## Done 2026-07-02 (packaging v1 + full docs)

- **Relocatable Linux bundle** (`experiments/package-linux.sh`): release
  binary + app/server + etc assets → dist/ (190MB, system ruby for now);
  ends with a relocation smoke test (boot real runtime from a moved dir,
  verified shutdown) — PASS. App root resolves at runtime
  (SONIC_OXIDE_APP_ROOT → exe-relative ../app → dev fallback).
- **Help pane full docs**: lang `doc:` bodies (multiline Ruby strings)
  parsed into `long_doc`; shown under the summary (1200-char cap).

## Done 2026-07-02 (controls a11y)

- **All 11 header controls are now real screen-reader buttons**: labeled
  AccessKit Button nodes with Click actions routed through a single `Cmd`
  dispatch shared with mouse handlers. AT-SPI verified: window exposes 15
  children (11 controls + 4 panes; editor Text interface intact).
- GOTCHA: the AccessKit adapter can NOT reactivate after
  ScreenReaderEnabled toggles off in-process — restart the app with the
  flag already on before a walk. And kill Orca with
  `pkill -f "usr/bin/orc[a]"` (bracket trick — plain patterns match the
  harness's own shell and kill it, exit 144).
- Decision: character_positions/widths for TextRuns stay deferred until we
  can source real glyph metrics from the editor layout — fabricated
  positions would actively mislead screen magnifiers.

## Done 2026-07-02 (music vocab + i18n)

- **Note/scale/chord completion** (the piano-helper substance): 153
  generated note symbols + scale/chord names parsed from scale.rb/chord.rb
  (real-repo test guards the parsers). Graphical piano/slider popup
  widgets deferred — they need custom completion-menu rendering
  (upstream gpui-component work).
- **i18n groundwork**: English-keyed translation table
  (gettext-compatible shape), etc/i18n/<lang>.conf via SONIC_OXIDE_LANG/
  LANG, per-key fallback; pane titles wired; demo de.conf shipped.

## Done 2026-07-02 (dock-lite complete, AppDir, i18n labels)

- **Resizable right column**: scope/cues/log in a v_resizable group —
  with click-to-collapse that's fold + resize done; only drag-to-REARRANGE
  remains of the dock story.
- **AppImage-ready AppDir** in package-linux.sh (AppRun/desktop/icon; own
  boot+verified-shutdown smoke test PASS — also covers the
  SONIC_OXIDE_APP_ROOT env path). `.AppImage` builds when appimagetool is
  installed.
- **i18n**: all 11 screen-reader control labels translate (de locale
  complete for controls + titles).

## ✅ MILESTONE: self-contained bundle (2026-07-02)

Packaging v2 complete — the bundle carries its OWN Ruby (interpreter +
stdlib + gems + libruby behind a path-pinning wrapper, official
server/native/ruby layout, preferred by paths.rs). No system-ruby
dependency. Smoke tests assert the FULL runtime (spider alive at quit)
and caught three real bundle bugs (etc/buffers, etc/synthdefs, VERSION
file × 2 layouts). Bundle 220MB + AppDir both PASS from relocated dirs.
Debugging aid: SONIC_OXIDE_DEBUG_CHILDREN=1 inherits child stdio.

## ✅ MILESTONE: SHIPPABLE ARTIFACT (2026-07-02)

`SonicOxide-x86_64.AppImage` — 71MB, single file, self-contained (bundled
Ruby, Spider, SuperSonic, all assets). Passes its own smoke test: full
runtime up from the mounted image, verified teardown. appimagetool
installs user-locally (no sudo): the release download in the AppImage
project's GitHub releases. i18n sweep complete for interactive UI text.

## ✅ DockArea LANDED (2026-07-02, revisiting the deferral)

The port turned out far cheaper than scoped: scope/cues/log are read-only
views, so they became Panel entities (macro boilerplate, observe-the-app
re-render) with zero listener re-plumbing. The right column is a real
DockArea — drag tabs to stack/rearrange, dividers to resize. Toggled
panes (settings/help/nodes) stay stacked above it. Verified: tests green,
lifecycle E2E clean; visual drag check is the user's.

## Remaining tail (nothing left that this machine + this codebase can do)

1. **Upstream** (gpui-component): completion-menu widgets (piano/slider),
   editor a11y glyph metrics — PRs/issues, external cadence.
2. **i18n**: .po migration + more locales (mechanism + de demo done).
3. **Packaging**: mac/windows scripts (need those machines). CI AppImage
   artifacts on every push: DONE.
4. **Qt retirement** per roadmap exit criteria — REQUIRES sustained
   side-by-side daily use (weeks). Both apps run; comparison enabled.

## Done 2026-07-02 (earlier session — 13 tests)

- **Clean shutdown implemented**: `Context::on_app_quit` → `Backend::shutdown()`
  → `session.shutdown()` (`/daemon/exit`) + `Daemon::wait_timeout(3s)` +
  `kill()` backstop; `impl Drop for Daemon` as final backstop;
  `cx.on_window_closed` quits the app when the last window closes (before,
  the process + daemon lingered). NOT yet runtime-verified — needs one
  app-quit while watching the process table.
- **Real scope wired**: new `ScopeSlotReader` in `rust-core::audio::shm`
  mirrors SuperSonic's fixed-inline triple-buffered scope slot
  (`server_shm.hpp::shm_scope_buffer_reader`; planar data, `stage` publishes).
  Probed live against a running engine: attaches, slot 0 ACTIVE, pulls
  1024-frame windows. KEY FACT: the audio *ring* (slot-0 recording tap) is
  IDLE on native — only WASM writes it from the post-block hook; native needs
  a `supersonic-audio-out` synth. The scope path is the live one. The spike
  attaches lazily (retry ~1s, segment appears after handshake) to
  `/SuperSonic_<scsynth port>` slot 0, falls back to the fake ring under
  loopback, demo sine until data flows.
- **Button feedback** (user feedback): header now shows ●playing/○idle;
  Stop ■ is danger-red while playing, outline when idle; Run/Stop clicks
  echo instantly into the Log; `playing` clears on `StatusType::AllComplete`
  from the spider (and optimistically on Stop click).
- `examples/scope_probe.rs` in rust-core probes a live segment (audio ring +
  scope slots): `cargo run --example scope_probe -- /SuperSonic_<port>`.

(The earlier session's priority list is superseded by "Next priorities" above;
the Phase-0 Qt copy fix still needs its one-time Qt build + smoke test.)

## Environment gotchas (will bite a fresh session)

- gpui-spike builds with **nightly 1.95** (`rust-toolchain.toml` handles it);
  rust-core is stable. First gpui build ≈ several minutes.
- GPUI pins: gpui-component @ `1505b14`, zed/gpui @ `1d217ee` — bump together.
- A **separate containerized Sonic Pi** runs under `/app` on this machine
  (different checkout) — ignore its processes.
- The a11y bus test flags: enable/disable `org.a11y.Status` `IsEnabled` +
  `ScreenReaderEnabled` via gdbus; always restore to false after.
- `git status`: `experiments/`, `plan/` are new/untracked; `mainwindow.cpp`,
  `sonicpilog.cpp` modified (Phase 0); submodule now populated. Nothing is
  committed yet — decide what to commit when the E2E run works.
