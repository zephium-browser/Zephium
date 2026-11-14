#!/usr/bin/env bash
set -euo pipefail

if (( $# == 0 )); then
  printf 'usage: %s COMMAND [ARG ...]\n' "$0" >&2
  exit 2
fi

test_session="$(mktemp -d)"
trap 'rm -rf "${test_session}"' EXIT
test_cargo_home="${CARGO_HOME:-${HOME}/.cargo}"
test_rustup_home="${RUSTUP_HOME:-${HOME}/.rustup}"
mkdir -p "${test_session}"/{home,config,data,cache,state,runtime}
chmod 700 "${test_session}/runtime"

test_bus_config="${test_session}/bus.conf"
cat > "${test_bus_config}" <<EOF_BUS
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=${test_session}/runtime</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF_BUS

env -u DBUS_SESSION_BUS_ADDRESS -u DBUS_STARTER_ADDRESS -u DBUS_STARTER_BUS_TYPE \
  -u DISPLAY -u WAYLAND_DISPLAY -u SESSION_MANAGER \
  HOME="${test_session}/home" CARGO_HOME="${test_cargo_home}" \
  RUSTUP_HOME="${test_rustup_home}" XDG_CONFIG_HOME="${test_session}/config" \
  XDG_DATA_HOME="${test_session}/data" XDG_CACHE_HOME="${test_session}/cache" \
  XDG_STATE_HOME="${test_session}/state" XDG_RUNTIME_DIR="${test_session}/runtime" \
  GDK_BACKEND=x11 NO_AT_BRIDGE=1 ZEPHIUM_WEBEXT_PROBE_PRIVATE_SESSION=1 \
  timeout -k 10 420 \
  xvfb-run --auto-servernum --server-args='-screen 0 1280x1024x24 -nolisten tcp' \
  dbus-run-session --config-file="${test_bus_config}" -- "$@"
