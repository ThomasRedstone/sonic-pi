#!/bin/bash
# Sonic Oxide — Linux bundle (Phase 4 packaging, v1).
#
# Stages a relocatable directory:
#   dist/sonic-oxide/bin/sonic-oxide     release binary
#   dist/sonic-oxide/app/server/...      Spider (Ruby sources) + SuperSonic
#   dist/sonic-oxide/etc/...             samples + doc sources (vocab/help)
#
# Uses the system ruby for now (daemon.rb-era releases bundle their own —
# that lands with the installer work). Ends with a relocation smoke test:
# the bundle must boot the real runtime via the supervisor and shut down
# verified, from a moved directory.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"
DIST="${SCRIPT_DIR}/dist/sonic-oxide"

echo "==> release build"
(cd "${SCRIPT_DIR}/gpui-spike" && cargo build --release --bin sonic-gpui-spike)

echo "==> staging ${DIST}"
rm -rf "${DIST}"
mkdir -p "${DIST}/bin" "${DIST}/etc"
cp "${SCRIPT_DIR}/gpui-spike/target/release/sonic-gpui-spike" "${DIST}/bin/sonic-oxide"

# Runtime: Spider + SuperSonic. Exclude logs and caches.
mkdir -p "${DIST}/app"
rsync -a --exclude 'log/' --exclude '*.log' --exclude '.git' \
  "${REPO}/app/server" "${DIST}/app/"

# Assets the frontend reads directly (samples, completion/help sources).
rsync -a "${REPO}/etc/samples" "${DIST}/etc/"
mkdir -p "${DIST}/etc/doc"
rsync -a "${REPO}/etc/doc/cheatsheets" "${DIST}/etc/doc/"

du -sh "${DIST}"

echo "==> relocation smoke test"
SMOKE="$(mktemp -d)/sonic-oxide"
cp -r "${DIST}" "${SMOKE}"
if SONIC_SPIKE_AUTOQUIT=15 timeout 90 "${SMOKE}/bin/sonic-oxide" 2>&1 \
    | grep -q "children stopped (verified)"; then
  echo "==> SMOKE TEST PASS (booted + verified shutdown from ${SMOKE})"
  rm -rf "$(dirname "${SMOKE}")"
else
  echo "==> SMOKE TEST FAIL" >&2
  rm -rf "$(dirname "${SMOKE}")"
  exit 1
fi

echo "==> bundle ready: ${DIST}"
