#!/bin/sh
# Installs a release package on a clean Debian, Ubuntu or Fedora system and
# checks it runs: linked and dlopened libraries resolve, --help works, and a
# launch gets as far as finding no display. Run as root in a container:
#
#   install-test.sh /abs/path/vidicon_amd64.deb
#   install-test.sh /abs/path/vidicon.x86_64.rpm
set -eu

pkg=$1
log=/tmp/install-test.log
case $pkg in
  *.deb) install() { apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y "$pkg"; } ;;
  *.rpm) install() { dnf install -y "$pkg"; } ;;
  *)
    echo "not a .deb or .rpm: $pkg"
    exit 1
    ;;
esac
install > "$log" 2>&1 || { tail -40 "$log"; exit 1; }

if ldd /usr/bin/vidicon | grep 'not found'; then
  exit 1
fi

# winit loads these at runtime, so ldd does not list them.
missing=0
for lib in libEGL.so.1 libGL.so.1 libwayland-client.so.0 libwayland-egl.so.1 \
  libxkbcommon.so.0 libxkbcommon-x11.so.0 libX11.so.6 libX11-xcb.so.1 \
  libXcursor.so.1 libXi.so.6; do
  ldconfig -p | grep -q "[[:space:]]$lib " || { echo "missing $lib"; missing=1; }
done
test $missing = 0

vidicon --help > /dev/null

out=$(timeout 30 vidicon 2>&1 || true)
case $out in
  *'neither WAYLAND_DISPLAY nor WAYLAND_SOCKET nor DISPLAY is set'*) ;;
  *)
    echo "$out"
    exit 1
    ;;
esac

# Ubuntu 22.04's Inter (3.19) has no Inter Display; the README says so.
echo "Inter Display faces: $(fc-list | grep -c 'Inter Display' || true)"
echo "ok: $(basename "$pkg")"
