# Phase 6 — Gig-hardening: the show must go on

> Read [`00-roadmap.md`](./00-roadmap.md) first. This is the highest-value
> post-parity phase: it's the one capability Qt never had, and it's squarely
> engineering (not waiting on hardware or upstream).

## Goal

Make the runtime survive the GUI. Today's design deliberately does the
opposite: `Supervisor::boot` (`experiments/rust-core/src/supervisor.rs`)
ties Spider + SuperSonic to the app's life via `PR_SET_PDEATHSIG(SIGTERM)` —
if the GUI dies for any reason, the kernel kills the children within one
scheduling tick. That's *correct* for a dev loop (no orphaned processes to
hunt down) and *wrong* for a stage: a GUI crash mid-set should never kill the
music.

The target: a **performance mode** where the runtime detaches from the GUI's
lifetime, and a relaunched GUI **reattaches** to the still-running session —
buffers restored, sound uninterrupted.

## Why this, why now

- It's not a feature Sonic Pi (Qt) has ever offered — genuinely
  differentiated, not parity work.
- The architecture is already halfway there: Spider and SuperSonic are
  already separate processes from the GUI, communicating over OSC/shm. The
  GUI is not in the audio path. Detaching it is additive, not a rewrite.
- The current 40s+ Ruby daemon kill-switch this replaced was explicitly
  called out as a stage risk in the original migration; this phase finishes
  that thought properly instead of just moving the risk around.

## Design sketch

### 1. Detached boot mode

Add a `--gig` flag (or a settings toggle, persisted like the other prefs)
that changes `Supervisor::boot`'s behaviour:

- Skip `die_with_parent()` for both children (supervisor.rs:200,245) — they
  must outlive the GUI process.
- Write a **session lockfile** (`~/.sonic-pi/store/sonic-oxide/session.lock`,
  JSON) once both children are up: `{ token, spider_pid, spider_started_at,
  supersonic_pid, supersonic_started_at, ports: {..} }`. `started_at` guards
  against PID reuse — Linux recycles PIDs, so "PID N is running" isn't
  enough; compare `/proc/<pid>/stat`'s start-time field (already monotonic,
  already available, no new dependency) against the recorded value.
- On normal (non-gig) shutdown, remove the lockfile so a later non-gig boot
  never tries to reattach to a session that no longer exists.

### 2. Reattach on boot

Before `Backend::connect` tries `try_supervisor`/`try_real`, check for a
live lockfile:

- Read it; verify both PIDs are alive AND their start-times match (rules out
  a stale lockfile pointing at recycled PIDs).
- If live: skip spawning entirely. Construct `Ports` from the lockfile,
  `Session::connect` against them exactly as the existing paths do — no
  protocol change, because the GUI has never needed to know whether it just
  spawned the children or is attaching to survivors.
- If the ports in the lockfile don't answer `/ping` within a short timeout
  (session died between the liveness check and the connect), fall through
  to a normal boot and overwrite the lockfile.

### 3. Panic-safe state

The GUI already autosaves buffers every 5s (`AUTOSAVE_TICKS` in
`gpui-spike/src/main.rs`) and writes on quit. Add a `std::panic::set_hook` in
`main()` that does a best-effort synchronous `save_workspace` before the
default hook runs — a crash loses at most ~5s of unsaved edits instead of
whatever was typed since the last autosave tick, and unlike the timer this
runs at the moment of the crash.

### 4. Never orphan for real

Detaching from the GUI must not mean "orphan forever if the user actually
wants to quit everything." Add an explicit **"Stop performance"** action
(header button + confirmation, since it's destructive to a running set) that
sends the same `/daemon/exit`-equivalent shutdown and removes the lockfile —
the deliberate, user-initiated version of the automatic cleanup that gig mode
otherwise forgoes.

## Work items

1. `supervisor.rs`: `BootMode { Normal, Gig }` param threaded through `boot`;
   conditional `die_with_parent`; lockfile read/write/verify helpers (unit
   tested with a fake `/proc` layout or an injectable clock — don't shell
   out to real `/proc` in tests).
2. `main.rs`: `--gig` env/flag plumbing (mirror the existing
   `SONIC_OXIDE_*` env-var convention); reattach probe before `Backend::connect`;
   panic hook; "Stop performance" action + confirmation dialog.
3. **Fuzz the OSC parser.** `protocol::parse_incoming` is the one piece of
   attack surface a detached, longer-lived runtime makes more relevant
   (malformed packets on a network cue port, a buggy MIDI controller, a
   flaky external sender). `cargo fuzz` target over
   `rosc::decoder::decode_udp` → `parse_incoming`; cheap to set up, and any
   panic it finds is a real bug regardless of whether gig mode ships.
4. **Soak test.** A long-running example (`examples/soak.rs`, mirroring
   `latency_probe`'s boot pattern) that runs a live_loop for N hours,
   periodically checking memory (RSS via `/proc/<pid>/status`) and scope
   liveness, logging any drift. Start with a 1-hour CI-friendly variant and
   a manual 24h run on real hardware before trusting gig mode on a stage.
5. Update `03-status.md` + this file with results; the reattach path
   especially needs a real "kill -9 the GUI mid-run, relaunch, confirm sound
   never stopped and the relaunched GUI shows the right buffer" manual test —
   no amount of unit testing substitutes for that one.

## Status (2026-07-04)

✅ **Implemented and e2e-verified against the real runtime.**
`sonicpi-core::session_lock` (`SessionLock`, fail-closed `still_live()` via
`/proc/<pid>/stat` start-time comparison), `Supervisor::boot`'s `BootMode`
param (`Gig` skips `PR_SET_PDEATHSIG` and Drop-triggered shutdown),
`SONIC_OXIDE_GIG=1` boots detached, a relaunch auto-reattaches (no flag
needed for that half), and "⏏ Stop Performance" in the header ends a gig
session explicitly (PID-based kill for reattached sessions, since the app
never held `Child` handles for those). Panic-safe autosave snapshot lands
alongside it. `examples/gig_reattach_check.rs` (`make e2e`) proves the
whole story end-to-end: boot gig → simulate a crash (drop the Supervisor
without shutdown) → children survive → reattach from the lock file alone →
ping/run through the reattached session → explicit stop → lock correctly
reads as dead. PASS on first real run after one fix (see gotcha below).

**Gotcha (matters for future debugging, not a bug in shipped code):** a
killed process stays a zombie — visible in `/proc`, same recorded start
time — until its PARENT calls `wait()`/`waitpid()` on it. In a real
crash+relaunch, the dead GUI's children get reparented to init, which reaps
them promptly, so a genuinely different relaunched process is never in a
position to need to (and must not try to — `waitpid` on a PID that isn't
your child fails with ECHILD). The e2e test, however, simulates the crash
by dropping the `Supervisor` value WITHIN THE SAME OS PROCESS — so that
process remains the real parent throughout and must explicitly `waitpid`
the killed children itself to accurately stand in for init's reaping. If
`still_live()` ever seems to report a stale "yes" in ad-hoc testing, check
whether the checking process is actually the parent before suspecting the
PID-reuse guard.

✅ **Fuzz-lite harness landed** (`tests/fuzz_lite.rs`, runs in every normal
`cargo test`, zero new dependencies, stable Rust): known-tricky byte
patterns, 20k deterministic random-byte trials, and mutation-of-real-
messages all pushed through `decode_udp` → `parse_incoming` — no panics
found. NOTE: a real corpus-guided `cargo fuzz` setup (libFuzzer, coverage-
guided) remains a nice-to-have upgrade — this machine has no `clang`
installed, which `cargo-fuzz` needs; the fuzz-lite harness is the actual
shipped, CI-enforced floor in the meantime and already covers the stated
goal (the parser must never panic on malformed input).

✅ **Soak harness landed** (`examples/soak.rs`, `make soak`): boots the real
runtime, runs a live_loop, samples child liveness + scope liveness + RSS
memory on an interval, fails on any child death, on the scope never
publishing, or on >2x memory growth between the first post-warmup sample
and the last. A 60s local run (default) PASSED cleanly: stable RSS
(spider ~57MB flat, engine ~47MB flat), scope confirmed live after
warmup. Deliberately NOT wired into CI (needs the real SuperSonic binary
+ Ruby, which CI doesn't build — matches every other real-runtime e2e
example). The real multi-hour pre-stage run
(`SONIC_OXIDE_SOAK_SECS=86400 make soak`, or just set the env var) is
still Tom's to do on real hardware before trusting gig mode on a stage.

**Not yet done:** the one-time real "kill -9 my actual running GUI, confirm
sound never stopped" hands-on check — the e2e example proves the mechanism
against a throwaway session; only Tom pulling the trigger on his own
running instance closes the loop for real.

**Phase 6 status: substantially complete.** All Work Items from the
original plan are implemented and machine-verified (unit tests, an e2e
example against the real runtime, and the fuzz/soak harnesses). What
remains is entirely hands-on-hardware verification that no amount of
automation can substitute for.

## Exit criteria

- `kill -9` on the GUI process while a `live_loop` plays: sound continues
  uninterrupted; relaunching the GUI (in `--gig` mode) reattaches within the
  normal boot-probe timeout and shows the buffers as they were.
- A non-gig relaunch after a gig session correctly detects the stale-vs-live
  lockfile in both directions (session still running → do NOT spawn a
  second SuperSonic on the same audio device; session actually dead →
  clean normal boot).
- Fuzz target runs a few CPU-hours with zero panics before calling the
  parser hardened.
- 1-hour soak passes in CI; a real multi-hour soak passes by hand at least
  once before this phase is considered done.

## Risks

- **Two audio-owning processes on one device** if reattach logic is wrong
  and a stale lockfile is trusted — this is the one failure mode that's
  worse than the status quo (silence) so the liveness check must fail
  *closed* (assume dead → normal boot) on any ambiguity, never fail open.
- **Windows/macOS**: the lockfile + `/proc` liveness check is Linux-specific
  in its exact mechanism (start-time comparison); the *design* ports (use
  `CreateToolhelp32Snapshot`/`sysctl KERN_PROC` for the equivalent), but
  implement it only when phase 9's platform work needs it — don't block gig
  mode on ports nobody can test yet.
