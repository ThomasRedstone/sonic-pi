# Rust core — Phase 2 scaffold

A Rust reimplementation of the C++ `app/api` layer (`SonicPiAPI`), designed to
sit behind the **same OSC + `IAPIClient` contract** so it can be validated with
the existing Qt GUI as an oracle. See
[`../../plan/02-implementation-plan.md`](../../plan/02-implementation-plan.md),
Phase 2.

Stable Rust — no nightly, no GPUI. Fast to build and test.

```sh
cd experiments/rust-core
cargo test              # protocol, ports, and OSC round-trip tests
cargo run --example contract_demo
```

## What's real vs. scaffolded

Real and tested:
- **`protocol`** — the OSC vocabulary (addresses + argument shapes) taken
  verbatim from `sonicpi_api.cpp` / `osc_handler.cpp`, outgoing builders
  (`run_buffer`, `stop_all_jobs`, `ping`, `keep_alive`, `daemon_exit`, …), and
  the incoming parser mapping messages → `ClientEvent`.
- **`ports`** — the daemon handshake parser (`<daemon> <gui_listen_to_spider>
  <gui_send_to_spider> <scsynth> <tau_osc_cues> <token>`).
- **`osc`** — localhost UDP sender + a listening server (send/receive tested).
- **`session`** — wires the three senders + incoming server + keep-alive loop
  behind ports/token; exposes `run` / `stop` / `ping` / `shutdown`.
- **`client`** — the Rust mirror of `IAPIClient` (collapsed to one `on_event`).

Skeletons with clear TODOs (need the full runtime to exercise):
- **`process`** — spawns `ruby daemon.rb` and reads the handshake; the flow is
  complete but not integration-tested.
- **`paths`** — server-relative joins; user/home/config/log dirs are TODO
  (→ `directories` crate).
- **`audio`** — the shm scope reader (`audio::shm`) is **real and folded in**
  from the Phase-1 spike (tested: layout, atomics, wrap); `AudioProcessor` wires
  it up. Still TODO: the `rustfft` spectrum stage + display ballistics that fill
  the rest of `ProcessedAudio`. The GPUI spike consumes this module.

## How this validates (Phase 2 plan)

The C++ GUI links `libsonicpi-api`. The drop-in path is a thin C ABI over this
crate exposing the same `SonicPiAPI` surface, swapped behind a build flag so the
existing GUI drives the Rust core. If Sonic Pi behaves identically, the core is
correct. Golden OSC traffic captured from the current app becomes the regression
suite for `protocol`.

## Contract sources

- `app/api/include/api/sonicpi_api.h` — API surface + `IAPIClient`
- `app/api/src/sonicpi_api.cpp` — outgoing OSC + daemon handshake
- `app/api/src/osc/osc_handler.cpp` — incoming OSC → callbacks
- `app/api/include/api/audio/shm_audio_buffer.hpp`, `server_shm.hpp` — scope shm
