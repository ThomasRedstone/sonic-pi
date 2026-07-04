# Phase 9 — Platforms & release

> Read [`00-roadmap.md`](./00-roadmap.md) first. This phase has two
> genuinely independent halves — **platform ports** (hardware-gated
> engineering) and **going public** (a product/community decision) — don't
> let one block the other in planning.

## Part A — mac/Windows apps

### Where this already stands

The cross-platform *core* is done and CI-verified (see `03-status.md`):
`rust-core` passes its full unit suite on macOS and Windows, including a
from-scratch Windows shared-memory backend
(`experiments/rust-core/src/audio/shm.rs`, `Mapping` for `cfg(windows)`)
written blind and verified by CI on the first real run. What's left is
building the *app* — packaging, and the two known design gaps that only
matter with a real GUI running.

### Known gaps (from `03-status.md` / `supervisor.rs`)

1. **Child crash-safety off Linux.** `die_with_parent()` in `supervisor.rs`
   is a no-op outside `target_os = "linux"` (the `PR_SET_PDEATHSIG` syscall
   has no direct equivalent elsewhere):
   - **macOS**: no single-syscall equivalent. Options: a `kqueue`
     `EVFILT_PROC`/`NOTE_EXIT` watcher on the child, held by a lightweight
     thread that kills the child the moment the parent's death is detected
     (requires the watcher to be a *third* process or a very careful
     same-process design, since the parent dying means the watching thread
     dies too) — realistically this means a tiny separate watchdog helper
     process, spawned alongside Spider/SuperSonic, whose only job is to
     `kqueue`-watch the main app's PID and kill the audio children if it
     exits. Same shape as gig-hardening's *opposite* case — worth building
     these two together since they touch the same code paths in opposite
     directions (phase 6 wants the children to survive the GUI in gig
     mode; this wants them to die with it in *normal* mode on non-Linux).
   - **Windows**: **Job Objects** (`CreateJobObject` +
     `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) are the direct, single-API
     equivalent of `PR_SET_PDEATHSIG` — assign both children to a job at
     spawn time; closing the job handle (which happens automatically when
     the process exits, if not held open elsewhere) kills everything in
     it. This is the easier of the two ports.
2. **Ruby + SuperSonic bundling per OS.** `experiments/package-linux.sh`
   bundles a self-contained Ruby (interpreter + stdlib + gems + libruby)
   and the built SuperSonic binary. Each OS needs its own packaging script:
   - **Windows**: bundle a Windows Ruby build (RubyInstaller's portable
     variant is the usual choice) + a Windows-built SuperSonic; package as
     an installer or a portable zip (there's no Windows AppImage
     equivalent — MSIX or a plain zip are the realistic options for a
     first cut).
   - **macOS**: an `.app` bundle (standard `Info.plist` + `Contents/{MacOS,
     Resources}` layout) wrapping the same components; code-signing and
     notarization are a real requirement if this is ever distributed
     outside a dev machine (unsigned `.app`s trigger Gatekeeper friction),
     but can be deferred for personal/dev use.
3. **GUI smoke on real hardware.** CI can't validate audio device access,
   window chrome, or the actual screen-reader story on mac/win — this
   needs hands-on time on real machines once they're available. Treat CI
   green as "the core is portable," not "the app works."

### Work items (ordered by dependency)

1. Windows Job Objects for crash-safety (self-contained, testable on a
   Windows CI runner even without a full GUI — spawn a dummy child, kill
   the "parent" test process, assert the child dies).
2. macOS watchdog-process design + implementation (more involved; sketch
   the IPC between watchdog and main app before writing code — likely just
   "watchdog exits when it detects the main process gone, having already
   killed the audio children," no ongoing communication needed).
3. Per-OS packaging scripts, modeled on `package-linux.sh`'s structure
   (asset rsync list, Ruby bundling, smoke test that boots the real
   runtime from the packaged layout before calling it done).
4. Hands-on GUI/audio smoke on real mac and Windows hardware.
5. Add the equivalent CI packaging jobs to `sonic-oxide-xplat.yml` once
   the scripts exist (mirroring the Linux package job's pattern) — but
   keep them off the default trigger path per the existing "sparing CI
   usage" policy; packaging jobs are expensive and only need to run on
   `rust-core`/packaging-script changes, not every push.

### Exit criteria

- A double-clickable app on both platforms that boots the real runtime,
  runs a `live_loop`, and shuts down cleanly (verified by a smoke test
  mirroring `make e2e`'s AUTOQUIT pattern, adapted per-OS).
- Killing the app process (not a graceful quit) leaves no orphaned
  Spider/SuperSonic processes on either platform — the actual point of
  this work.

## Part B — going public (a decision, not a build)

This half has no code prerequisites — it can happen whenever Tom decides
it's worth having the conversation, independent of Part A's progress.

### Questions to answer, not solve unilaterally

- **Where does this live?** Options, roughly in order of how much they
  change the current setup: (a) stays exactly as-is, a personal fork
  nobody else needs to know about; (b) gets a README + is left discoverable
  on GitHub for anyone who stumbles on it, no active promotion; (c) is
  proposed to the upstream Sonic Pi project as an alternative/future GUI;
  (d) becomes its own named, independently-released project. Each has a
  different amount of ongoing obligation (docs, issue triage, compat
  promises) — worth being honest about appetite before choosing.
- **Release mechanics**, if any option beyond (a) is chosen: versioning
  scheme, a CHANGELOG, GitHub Releases with the AppImage/mac/win artifacts
  attached, maybe an update-checker (SuperSonic's OSC API already has
  `/enable-update-checking` plumbed on the Spider side, but that's Sonic
  Pi's own PyPI-style version check, not an Oxide release feed — a new,
  separate mechanism if wanted).
- **Distribution channels**, if wanted: Flatpak (Flathub) and an AUR
  package are the natural low-effort options for Linux once the AppImage
  exists — both are largely "write a manifest pointing at the existing
  build," not new engineering.

### Recommendation

Don't decide this now. Revisit once phase 6 (gig-hardening) and a real
Tier-1 exit rehearsal have happened — "would I show this to someone else"
is a much easier question to answer once it's been gigged, not just
tested.
