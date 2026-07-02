# Building Sonic Oxide

Everything routes through `experiments/Makefile`:

```
make deps      # names any missing system packages (with the apt line to fix)
make run       # build + launch (Rust supervisor mode by default)
make test      # both crates (rust-core stable, gpui-spike pinned nightly)
make coverage  # rust-core ~95% lines (CI gates at >=90)
make e2e       # boots the REAL runtime: supervisor_check + record_check
make package   # bundle + AppDir + AppImage, each smoke-tested
make qt        # the Qt oracle, one-time (self-heals ruby_help.h)
```

## Layout

| Piece | Where | Toolchain |
|---|---|---|
| `sonicpi-core` (protocol/session/supervisor/shm/spectrum) | `rust-core/` | stable |
| Sonic Oxide app | `gpui-spike/` | nightly, pinned by `rust-toolchain.toml` (GPUI needs 1.95 APIs) |
| Packaging | `package-linux.sh` → `dist/` | — |

`cargo run` in `gpui-spike/` just works (`default-run`). rustup switches
toolchains automatically per directory. First GPUI build takes several
minutes; after that it's seconds.

## Runtime knobs (env vars)

| Var | Effect |
|---|---|
| `SONIC_OXIDE_DAEMON=1` | boot via classic daemon.rb instead of the supervisor (A/B oracle) |
| `SONIC_OXIDE_APP_ROOT` | where the Sonic Pi `app/` runtime lives (bundles set this) |
| `SONIC_OXIDE_RUBY` | explicit ruby interpreter (else bundled, else system) |
| `SONIC_OXIDE_LANG` | UI language (else `LANG`; locales in `etc/i18n/`) |
| `SONIC_OXIDE_DEBUG_CHILDREN=1` | inherit Spider/SuperSonic stdio (debugging) |
| `SONIC_SPIKE_AUTOQUIT=<secs>` | quit via the clean-shutdown path (CI/smoke hook) |
| `SONIC_OXIDE_BOOT_TIMEOUT_SECS` | engine-readiness wait (tests use stubs + 1s) |

## Packaging output

`make package` produces, each with a boot-the-real-runtime + verified-shutdown
smoke test:

- `dist/sonic-oxide/` — relocatable directory (bundled Ruby; no system deps)
- `dist/SonicOxide.AppDir/` — AppImage staging tree
- `dist/SonicOxide-x86_64.AppImage` — single-file distributable (needs
  `appimagetool` on PATH; a user-local install from the AppImage project's
  GitHub releases works — no sudo)

`SKIP_SMOKE=1` builds artifacts without booting (CI runners have no
display/audio). CI uploads the AppImage artifact on every push.

## The Qt oracle

`make qt` builds the original GUI for side-by-side parity checks. It needs
(one-time): `sudo apt install libaubio-dev libqscintilla2-qt6-dev qt6-base-dev
qt6-tools-dev qt6-svg-dev libqt6opengl6-dev`, plus the SuperSonic binary at
`app/server/native/supersonic` (see plan/03-status.md). `linux-config.sh` now
generates `gui/utils/ruby_help.h` itself when missing — previously a cryptic
cmake failure.
