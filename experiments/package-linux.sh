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

# Assets: what the frontend reads (samples, completion/help sources, i18n)
# plus what SPIDER reads at boot (buffers: rand-stream.wav; synthdefs).
rsync -a "${REPO}/etc/samples" "${DIST}/etc/"
rsync -a "${REPO}/etc/buffers" "${DIST}/etc/"
rsync -a "${REPO}/etc/synthdefs" "${DIST}/etc/"
mkdir -p "${DIST}/etc/doc"
rsync -a "${REPO}/etc/doc/cheatsheets" "${DIST}/etc/doc/"
rsync -a "${REPO}/etc/doc/tutorial" "${DIST}/etc/doc/"   # tutorial browser
rsync -a "${REPO}/etc/examples" "${DIST}/etc/"           # examples loader
rsync -a "${REPO}/etc/i18n" "${DIST}/etc/" 2>/dev/null || true

# ── Bundle ruby (packaging v2): the interpreter Spider runs on ships in the
# official layout (server/native/ruby/bin/ruby), which the core prefers over
# system ruby. A wrapper pins RUBYLIB/GEM_PATH/LD_LIBRARY_PATH to the copies.
echo "==> bundling ruby ($(ruby -v | cut -d' ' -f1-2))"
RUBY_REAL="$(ruby -e 'print RbConfig.ruby')"
RUBYLIBDIR="$(ruby -e 'print RbConfig::CONFIG["rubylibdir"]')"
ARCHDIR="$(ruby -e 'print RbConfig::CONFIG["rubyarchdir"]')"
LIBDIR="$(ruby -e 'print RbConfig::CONFIG["libdir"]')"
GEMDIR="$(ruby -e 'print Gem.default_dir')"
RB="${DIST}/app/server/native/ruby"
rm -rf "${RB}"
mkdir -p "${RB}/bin" "${RB}/lib/ruby/stdlib" "${RB}/lib/ruby/arch" "${RB}/gems"
cp "${RUBY_REAL}" "${RB}/bin/ruby.real"
rsync -a "${RUBYLIBDIR}/" "${RB}/lib/ruby/stdlib/"
rsync -a "${ARCHDIR}/" "${RB}/lib/ruby/arch/"
cp -a "${LIBDIR}"/libruby.so* "${RB}/lib/" 2>/dev/null || true
[ -d "${GEMDIR}" ] && rsync -a "${GEMDIR}/" "${RB}/gems/default/"

cat > "${RB}/bin/ruby" <<'RUBYWRAP'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
RB="$(dirname "${HERE}")"
export RUBYLIB="${RB}/lib/ruby/stdlib:${RB}/lib/ruby/arch${RUBYLIB:+:$RUBYLIB}"
export GEM_HOME="${RB}/gems/default"
export GEM_PATH="${RB}/gems/default"
export LD_LIBRARY_PATH="${RB}/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "${HERE}/ruby.real" "$@"
RUBYWRAP
chmod +x "${RB}/bin/ruby"

# The bundled interpreter must at least run and see its stdlib + gems.
"${RB}/bin/ruby" -e 'require "set"; require "socket"; print "bundled ruby ok: ", RUBY_VERSION' \
  || { echo "bundled ruby self-check FAILED" >&2; exit 1; }
echo

cp "${REPO}/VERSION" "${DIST}/"   # Spider reads ../../../../../VERSION at boot
du -sh "${DIST}"

if [ "${SKIP_SMOKE:-0}" = "1" ]; then
  echo "==> SKIP_SMOKE=1 — artifacts only (no display/audio, e.g. CI)"
fi

run_smoke() { [ "${SKIP_SMOKE:-0}" != "1" ]; }

echo "==> relocation smoke test (bundled ruby, full runtime)"
if run_smoke; then
SMOKE="$(mktemp -d)/sonic-oxide"
cp -r "${DIST}" "${SMOKE}"
OUT="$(SONIC_SPIKE_AUTOQUIT=18 timeout 90 "${SMOKE}/bin/sonic-oxide" 2>&1 || true)"
rm -rf "$(dirname "${SMOKE}")"
# Spider alive at quit proves the BUNDLED interpreter booted the language
# runtime; verified shutdown proves teardown.
if echo "${OUT}" | grep -q "spider alive: true, engine alive: true" \
    && echo "${OUT}" | grep -q "children stopped (verified)"; then
  echo "==> SMOKE TEST PASS (full runtime up on bundled ruby + verified shutdown)"
else
  echo "==> SMOKE TEST FAIL:" >&2
  echo "${OUT}" | tail -5 >&2
  exit 1
fi

fi
echo "==> bundle ready: ${DIST}"

# ── AppImage staging (packaging v2) ──────────────────────────────────────────
# Builds an AppDir around the bundle; produces a .AppImage when appimagetool
# is on PATH, otherwise leaves the AppDir ready for it.
APPDIR="${SCRIPT_DIR}/dist/SonicOxide.AppDir"
echo "==> staging ${APPDIR}"
rm -rf "${APPDIR}"
mkdir -p "${APPDIR}/usr"
cp -r "${DIST}/bin" "${DIST}/app" "${DIST}/etc" "${APPDIR}/usr/"
cp "${DIST}/VERSION" "${APPDIR}/usr/"
cp "${REPO}/app/gui/images/icon-smaller.png" "${APPDIR}/sonic-oxide.png"

cat > "${APPDIR}/sonic-oxide.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Sonic Oxide
Comment=Live coding music synthesis (Sonic Pi, oxidised)
Exec=sonic-oxide
Icon=sonic-oxide
Categories=AudioVideo;Audio;Development;
Terminal=false
DESKTOP

cat > "${APPDIR}/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export SONIC_OXIDE_APP_ROOT="${HERE}/usr/app"
exec "${HERE}/usr/bin/sonic-oxide" "$@"
APPRUN
chmod +x "${APPDIR}/AppRun"

if run_smoke; then
echo "==> AppDir smoke test"
APPOUT="$(SONIC_SPIKE_AUTOQUIT=18 timeout 90 "${APPDIR}/AppRun" 2>&1 || true)"
if echo "${APPOUT}" | grep -q "spider alive: true, engine alive: true" \
    && echo "${APPOUT}" | grep -q "children stopped (verified)"; then
  echo "==> APPDIR SMOKE TEST PASS (full runtime on bundled ruby)"
else
  echo "==> APPDIR SMOKE TEST FAIL" >&2
  exit 1
fi

fi
if command -v appimagetool >/dev/null 2>&1; then
  echo "==> building AppImage"
  (cd "${SCRIPT_DIR}/dist" && ARCH=x86_64 appimagetool SonicOxide.AppDir SonicOxide-x86_64.AppImage)
  if run_smoke; then
  echo "==> AppImage smoke test"
  IMGOUT="$(SONIC_SPIKE_AUTOQUIT=18 timeout 90 "${SCRIPT_DIR}/dist/SonicOxide-x86_64.AppImage" 2>&1 || true)"
  if echo "${IMGOUT}" | grep -q "spider alive: true, engine alive: true" \
      && echo "${IMGOUT}" | grep -q "children stopped (verified)"; then
    echo "==> APPIMAGE SMOKE TEST PASS"
  else
    echo "==> APPIMAGE SMOKE TEST FAIL" >&2
    exit 1
  fi
  fi
  echo "==> AppImage: ${SCRIPT_DIR}/dist/SonicOxide-x86_64.AppImage"
else
  echo "==> appimagetool not found — AppDir is ready at ${APPDIR}"
fi
