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

## THE PLAN from here (2026-07-03; parity audit: loop ~90%, product ~60%)

### Tier 1 — the live-coding loop to 100% (in order)

1. ✅ **Master volume slider** (2026-07-03) — header slider → `/mixer-amp`
   (token, amp 0..2, silent — `changeSystemPreAmp` wire format, loopback-
   asserted).
2. ✅ **Editor font size** (2026-07-03) — Ctrl+=/-/0, 8..40 clamp, persisted
   in prefs.conf (`store::load_prefs`/`save_prefs`, round-trip tested).
3. ✅ **File open/save** (2026-07-03) — native dialogs (Ctrl+O open into
   active buffer, Ctrl+S save [reuses the buffer's known path], Ctrl+Shift+S
   save-as; 📂/💾 header buttons, a11y-wired). Errors land red in the Log.
4. ✅ **Scope modes** (2026-07-03) — mono/stereo/mirror/Lissajous/spectrum,
   one header button cycles. `ScopeSlotReader::pull_latest_stereo` in
   rust-core (planar ch1; mono slots mirror ch0) — synthetic-segment tested.
   Loopback fake ring mirrors L→R. Demo signal is sin/cos (Lissajous circle).
5. ✅ **Editor built-ins audit** (2026-07-03) — undo/redo/find were already
   live: `gpui_component::init` binds ctrl-z/ctrl-y/ctrl-f in the focused-
   Input context, and `code_editor()` sets `searchable` (find+replace panel).
   Added the missing conventional alias: ctrl-shift-z → Redo. Hands-on
   verification folded into the Tier-1 exit rehearsal.

Exit test for Tier 1: a full performance rehearsal on Oxide with the Qt app
closed, no reach-for-the-other-app moments. **STILL PENDING — the clock
hasn't started.**

### Tier 2 — product gaps (rough order)

1. ✅ **Tutorial + examples browser** (2026-07-03) — 📖 pane: all 85
   chapters (filename order, titles from first line) → gpui-component
   `text::markdown` view; 34 examples by category, click loads into the
   active buffer. Loaders in `tutorial.rs`, real-repo tested.
   package-linux.sh now ships etc/doc/tutorial + etc/examples.
2. ✅ **Settings breadth** (2026-07-03) — driver picker (live
   `/supersonic/drivers/*`, daemon-brokered in daemon.rb mode), MIDI
   enable + default channel, network OSC (cue server on/off + remote),
   invert stereo / force mono, safe mode + timing guarantees + external
   synths (via the Qt `#__nosave__` run preamble — spider subtracts those
   lines from error line numbers), theme catalogue
   (`ThemeRegistry::watch_dir` on etc/themes; Sonic Pi Dark/Light shipped),
   language picker (live reload). All prefs persist in prefs.conf;
   spider-side ones re-apply on SpiderReady. Wire formats mirrored from
   mainwindow.cpp/spider-server.rb (see the wire-format table in that
   research if needed: MIDI + cue toggles are token-prefixed spider OSC;
   safe mode/MIDI channel are preamble-only).
   KEY MECHANISM: `/supersonic/drivers/list` replies to the request's
   SOURCE socket — `OscServer::send_from` sends from the listening socket
   (a fire-and-forget sender would lose the reply).
3. ✅ **Session/file management** (2026-07-03) — recent-files strip (cap 8,
   deduped, persisted), buffer tabs show their file names, per-buffer
   paths persist so Ctrl+S works across restarts.
4. ✅ **i18n** (2026-07-03) — full string sweep (everything user-visible
   with i18n access goes through tr; EditorA11y's static label is the one
   holdout), `experiments/i18n-tools.py` (extract / missing / to-po /
   from-po, CI-friendly), French locale shipped, unit test enforces de/fr
   key parity. BUILD.md documents the workflow.
5. **mac/windows** — build scripts + runtime paths per OS (needs machines).
6. **Upstream gpui-component** — completion-menu custom widgets
   (piano/slider), editor glyph metrics for magnifier a11y.

## Done 2026-07-03 (Tier 2.2–2.4; 33 spike + 37 core tests)

- rust-core grew: settings-toggle protocol builders (wire-format tested),
  `AudioDrivers`/`DriverSwitched` events, `Session` setting methods,
  `OscServer::send_from` (source-routed request/reply).
- Runtime-verified post-rehearsal: `make e2e` PASS (supervisor + record)
  and AUTOQUIT smoke clean on the settings build. Gotcha: an e2e run in
  the first seconds after an app quit can FAIL transiently — the audio
  device is still settling; rerun before diagnosing. Driver switching +
  MIDI/cue toggles still want a HANDS-ON check (Log echoes each send).
- Themes: sparse ThemeConfig JSONs inherit unset colors from the mode's
  defaults; `Theme::apply_config` + `Theme::change(cfg.mode, …)` applies.
  The catalogue only lists after watch_dir's async load lands.

## Done 2026-07-03 (small parity gaps; supersonic bump committed)

- SuperSonic submodule pointer committed (c4304ce2a, MAGIC verified).
- Mixer **HPF/LPF** (toggle + MIDI-note cutoff steppers 0..135,
  `/mixer-hpf|lpf-enable [token, freq]` / `-disable [token]`), **log cues**
  (preamble `use_cue_logging false`, Qt order: nearest user code),
  **gamepad** + **update checking** toggles (update checks default OFF —
  deliberate divergence from Qt; network opt-in), all persisted +
  re-applied on SpiderReady.
- **Recording stop now prompts where to save** (Qt parity); cancel keeps
  the take in the recordings store. rename with copy+remove fallback.
- **View toggles** in settings show/hide the scope/cues/log dock panes via
  `Panel::visible` — no dock surgery, panels keep state while hidden.
- NOTE: parity gaps NOT ported, deliberately: per-port MIDI enable,
  per-device gamepad enable (both niche; wire formats are in the research
  table), Qt's log-cues auto-scroll semantics.
- Runtime smoke deferred (app was in use); next boot exercises it.

### The gate

**Qt retirement** (roadmap exit criteria): sustained side-by-side daily use
after Tier 1 lands. Tier 1's exit test starts that clock.

## THE PLAN v2 — foundation phase (2026-07-03, Tiers 1+2 done)

Agreed order (Tom): harness → cross-platform → editor.

1. ✅ **Conformance + latency harness** (2026-07-03) —
   a) `latency_probe` (`make latency`, release build) BASELINE — release ≈
      debug (boot 2.2s Ruby-dominated; scope 425ms = engine publish
      cadence; acks <5ms): the core's latency is NOT build-profile-bound,
      release mainly buys GPUI render smoothness. `make run` is release
      now (`run-debug` for iteration). Editor FOLDING was already enabled
      (code_editor default; tree-sitter chevrons in the gutter — mouse
      only, no upstream keybindings yet). AppImage regenerated with the
      full feature set (71MB, smoke PASS). Original debug numbers:
      boot→spider-ready **~1.8s** (the "slow Ruby boot" assumption was
      wrong — YJIT/bytecode side-quest deprioritised; system ruby lacks
      YJIT anyway), run→first-reply **<5ms** (probe resolution),
      run→scope-active **~425ms**, cue→event **<5ms**. Ceilings stay
      generous for CI variance; ratchet after more samples.
   b) `conformance_check` (in `make e2e`): full settings-sweep vs the real
      runtime — PASS, zero token rejections; drivers list + switch round-
      trip live. Found+fixed pre-run: switch replies route to the request's
      source socket → must send via `OscServer::send_from` (fire-and-forget
      sender loses the reply).
   c) Optional later: automated Qt A/B replay — do it while Qt is still
      installed and trusted; that window closes at retirement.
2. ✅ **Cross-platform core** (2026-07-03) — rust-core is green on ALL
   THREE OSes in CI (`sonic-oxide-xplat.yml`): full unit suite on macOS
   and Windows, including the **CreateFileMapping shm backend** (written
   blind, `cargo check --target x86_64-pc-windows-msvc` locally first —
   works without the MSVC linker — then verified by the shm test suite on
   a real Windows runner, first try). `Mapping` is platform-split
   mirroring server_shm.hpp; `segment_exists` (shm_open / named-section
   probe) makes the supervisor readiness wait portable; windows-sys is a
   target-gated dep. CI USAGE IS DELIBERATELY SPARING: xplat triggers only
   on `experiments/rust-core/**` or manual dispatch (macOS bills 10x,
   Windows 2x Linux); both workflows cancel superseded runs per ref.
   STILL OPEN for real ports: child crash-safety off Linux (mac needs a
   watchdog/kqueue design, win Job Objects), Ruby+SuperSonic bundling per
   OS, GUI smoke on hardware.
3. **Editor foundation** (upstream gpui-component): glyph metrics
   (magnifier a11y), completion-widget API (piano/slider), folding,
   bracket matching, multi-caret — prioritise by what daily use surfaces.
   ✅ GLYPH METRICS IMPLEMENTED + AT-SPI VERIFIED (2026-07-03): TextRun
   nodes carry bounds + character_positions/widths (physical px) for
   visible lines. **GetCharacterExtents answers real rects** — monospace
   grid confirmed (~10.5px advance, 25px line step across a line break).
   GOTCHA: accesskit_consumer returns EMPTY character rects when a run
   lacks `text_direction` (its text.rs bails to Some(vec![])) — the
   AT-SPI reply is silently `-1 -1 -1 -1`. Always set
   set_text_direction(LeftToRight) on TextRun nodes.
   Cost gating: geometry is only computed after a screen reader has
   actually read the editor node (write_a11y_info doubles as the activity
   signal — GPUI only calls it while a11y is active; latched, one-frame
   lag). GPUI sets default node bounds BEFORE write_a11y_info, so our
   explicit bounds survive (element.rs:466-472). REMAINING: the AT-SPI
   walk to verify magnifier tracking — needs the Orca session
   (SPEAKS ALOUD — coordinate with Tom), plus a cost measurement with a
   full screen of text.
   Original finding: glyph metrics need NO upstream change —
   `InputState::range_to_bounds(byte_range) -> Option<Bounds<Pixels>>` is
   public (window coords; None off-viewport, a natural visible-lines
   gate), plus public `line_height()`. Plan: per-char bounds for visible
   runs → AccessKit `character_positions`/`character_widths`. Open
   questions before building: (a) per-frame cost (~4k range_to_bounds
   calls on a full screen — measure), (b) whether GPUI clobbers node
   bounds set in write_a11y_info (runs are zero-size boxes — explicit
   bounds must survive), (c) AT-SPI verify needs the Orca session
   (speaks aloud — coordinate with Tom). The piano/slider completion
   widgets remain genuinely upstream-blocked (completion-menu rendering
   API).

Spider-boot side-quest (cheap, benchmark first): MRI 3.3 `--yjit` flag on
the bundled ruby; YARV bytecode precompile (bootsnap-style) for the
require-heavy boot; pre-warmed fork later. JRuby evaluated and REJECTED:
solves throughput/parallelism we don't need (audio lives in SuperSonic),
worsens boot time + bundle size, C-ext/FFI risk.

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

## Done 2026-07-03 (Tier 1 complete + tutorial; 29 spike + 34 core tests)

- **Live-use fixes** (user feedback): dock tabs said "Unnamed" — the Panel
  trait's default `title()`; the oxide_panel! macro now takes a title (via
  i18n) and sets `closable(false)` (no re-open affordance exists yet).
  **Copy All** buttons on Cues + Log (selection copying is useless while
  set_value rewrites the pane); Log pane now scrolls the whole 200-line
  scrollback (newest-first, so the latest row needs no scrolling).
- Tier 1.3/1.4/1.5 + Tier 2.1 — see the plan lists above.
- Gotcha: `gen` is a reserved keyword in edition 2024 — rustc suggests
  r#gen; just name closures something else.
- SuperSonic submodule sits at c4304ce2a in the working tree (uncommitted,
  Tom's) — shm MAGIC re-checked vs server_shm.hpp: still 0x5C09E006, matches
  rust-core. Commit the pointer when convenient.
- Remotes: origin = ThomasRedstone/sonic-pi (the fork, push target),
  upstream = sonic-pi-net/sonic-pi.

## Done 2026-07-04 (help parity, tutorial images, launcher, highlighting)

- Help pane = Qt help tabs: category browser (Synths/FX/Samples/Functions/
  Notes/Scales/Chords), markdown doc pages (uncapped), structured OptDoc
  parse — which also fixed completions' silently-empty opt defaults.
- Tutorial diagrams render: gpui-component FORK
  (ThomasRedstone/gpui-component @ local-image-paths) patches markdown
  images with scheme-less URLs to load from disk; chapter loader
  absolutises relative paths (real-repo test: all 26 images resolve).
  UPSTREAM PR PENDING — the fork is our only divergence; drop the pin
  back to longbridge once merged.
- `make install-launcher`: desktop menu entry → release binary wrapper.
- GOTCHA (dep bumps WILL re-hit this class of bug): gpui-component ships
  NO default features — grammars are opt-in, and `code_editor("ruby")`
  SILENTLY renders plain text without `tree-sitter-ruby`. Syntax
  highlighting was off from day one and nobody noticed until 2026-07-04.
  When a feature "ships with the widget" but doesn't appear, check the
  feature flags before the code.

## ✅ PHASE 6 (gig-hardening) SUBSTANTIALLY COMPLETE (2026-07-04)

Full implementation + machine verification of `plan/04-gig-hardening.md`:
`session_lock` module (fail-closed PID-reuse-safe reattach gate),
`BootMode::Gig` (skips PDEATHSIG + Drop-triggered shutdown),
`SONIC_OXIDE_GIG=1` detached boot, auto-reattach on relaunch, explicit
"⏏ Stop Performance" action, panic-safe autosave snapshot, a fuzz-lite OSC
harness (zero new deps, runs in every `cargo test`), and a soak-test
harness (`make soak`) — PASS on a 60s local run (stable RSS, scope live).
The capstone: `examples/gig_reattach_check.rs` (`make e2e`) proves the
WHOLE story end-to-end against the real Spider+SuperSonic runtime — boot
gig, simulate a crash, confirm survival, reattach from the lock file
alone, run through the reattached session, explicit stop, confirm the
lock reads dead. PASS. Surfaced one genuine OS-mechanics subtlety (zombie
reaping only applies to whoever is the real parent) — documented in
`04-gig-hardening.md` so it doesn't confuse future debugging.
**Only remaining item**: Tom's own hands-on "kill -9 my real running GUI,
confirm sound never stopped" check, plus a real multi-hour soak on
hardware before trusting this on an actual stage — neither is
automatable.

## ✅ PHASE 7 (teaching mode) COMPLETE (2026-07-04)

Tutorial chapters now teach: `tutorial::extract_code_blocks` (unit-tested
against every real chapter) feeds a "▶ Example N" strip under each
chapter's markdown; clicking loads that snippet into a dedicated
`scratch` buffer (never the user's 10 numbered ones) and runs it through
the same, unmodified pipeline via a new `current_editor()`/
`current_run_name()` generalization (which also fixed a latent bug: Align/
Comment while viewing scratch would have silently hit the wrong buffer).
First-run: a brand-new profile (precise check —
`!store_dir.join("prefs.conf").exists()`) auto-opens straight into chapter
1; the empty state grew a "▶ Start here" button. New `SONIC_OXIDE_STORE_DIR`
env override let this get a REAL e2e verification (AUTOQUIT boot against a
throwaway dir — spider+engine alive, clean shutdown) without ever touching
Tom's actual workspace. See `05-teaching-mode.md` for the one scope
deviation (inline overlay buttons → a below-markdown strip; equivalent
functionality, `range_to_bounds` turned out to be editor-only, not
available on the markdown TextView).

## ◕ PHASE 10 (editor power) — audit done, hover docs shipped (2026-07-04)

Audited before building (per the plan's explicit instruction, given the
folding/highlighting gotcha from earlier): **hover docs** turned out to be
another near-free win — `gpui-component` ships a `HoverProvider` trait
mirroring the `CompletionProvider` we already use, so `SonicHover` (+ a new
`vocab::word_at` pure helper, unit tested) now shows the Help pane's
markdown doc for any synth/fx/sample/fn/opt name on hover, in every
buffer and the tutorial scratch buffer. **Multi-caret and keyboard fold
toggle are CONFIRMED (not assumed) upstream-blocked** — grepped the
source: no multi-cursor trace anywhere; fold mutation lives behind a
`pub(super)` field with no public entry point. Inline diagnostic-on-hover
has a viable path (SonicHover could check diagnostics too) but needs
mirrored state to reach them from the provider trait — scoped as a
deferred follow-up, not attempted this pass. See `08-editor-power.md` for
the full audit writeup.

## Post-parity phases (2026-07-04)

Loop/product parity (Tiers 1-2) and the foundation workstream (harness,
cross-platform core, editor a11y) are DONE. Five follow-on phases are now
planned in numbered files, referenced from `00-roadmap.md`'s "Post-parity
phases" table: `04-gig-hardening.md`, `05-teaching-mode.md`,
`06-performance-ui.md`, `07-platforms-release.md`, `08-editor-power.md`.
Recommended order: gig-hardening first (most differentiated, purely
engineering); teaching-mode alongside it; performance-ui and editor-power
are driven by what daily use surfaces, not a fixed schedule; platforms
half of phase 9 starts whenever mac/win hardware exists, the release half
is a standing decision to revisit later, not a task.

Also still open (see above): the `local-image-paths` gpui-component fork
patch should be PR'd upstream — Tom's call on when, five-minute action via
`gh`.

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
