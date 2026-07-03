# Sonic Oxide — one entry point for the whole dev loop.
#
#   make deps      check (and name) required system packages
#   make run       build + launch the app (supervisor mode by default)
#   make test      both crates' suites (77 tests)
#   make coverage  rust-core line coverage (gated >=90 in CI; ~95 today)
#   make package   self-contained bundle + AppDir + AppImage, all smoke-tested
#   make e2e       full-runtime checks against the real engine
#   make latency   boot/run/scope/cue latency baseline vs ceilings
#   make qt        one-time Qt oracle build (auto-generates ruby_help.h)
#
# Toolchains: rust-core builds on stable; gpui-spike pins its own nightly via
# rust-toolchain.toml — rustup handles both automatically.

SHELL := /bin/bash
ROOT  := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
REPO  := $(abspath $(ROOT)/..)

APT_DEPS := libxkbcommon-x11-dev libwayland-dev libvulkan-dev \
            libasound2-dev libjack-jackd2-dev

.PHONY: deps run test coverage package e2e latency qt clean

deps:
	@missing=""; \
	for p in $(APT_DEPS); do dpkg -s $$p >/dev/null 2>&1 || missing="$$missing $$p"; done; \
	for b in ruby rsync cc; do command -v $$b >/dev/null 2>&1 || missing="$$missing $$b"; done; \
	if [ -n "$$missing" ]; then \
	  echo "missing:$$missing"; \
	  echo "dev libs: sudo apt install$$missing  (ruby/rsync/cc via your usual packages)"; \
	  exit 1; \
	else echo "all system deps present"; fi

run:
	cd $(ROOT)/gpui-spike && cargo run

test:
	cd $(ROOT)/rust-core && cargo test --all-targets
	cd $(ROOT)/gpui-spike && cargo test

coverage:
	cd $(ROOT)/rust-core && cargo llvm-cov --all-targets --summary-only
	cd $(ROOT)/gpui-spike && cargo llvm-cov --summary-only

package:
	$(ROOT)/package-linux.sh

# Full-runtime end-to-end checks (need audio + the built SuperSonic).
e2e:
	cd $(ROOT)/rust-core && cargo run --example supervisor_check
	cd $(ROOT)/rust-core && cargo run --example record_check

# Latency baseline (foundation harness): prints the numbers that define the
# "live" feel and fails past its ceilings — ratchet them down over time.
latency:
	cd $(ROOT)/rust-core && cargo run --example latency_probe

# The Qt oracle. linux-config.sh now self-heals the ruby_help.h footgun.
qt:
	cd $(REPO)/app && ./linux-config.sh && ./linux-build-gui.sh
	@echo "Qt binary: $(REPO)/app/build/gui/sonic-pi"

clean:
	cd $(ROOT)/rust-core && cargo clean
	cd $(ROOT)/gpui-spike && cargo clean
	rm -rf $(ROOT)/dist
