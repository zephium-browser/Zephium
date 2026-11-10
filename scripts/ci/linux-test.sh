#!/usr/bin/env bash
# Run native tests on a disposable display/session without the user's data or bus.
set -euo pipefail

sandbox="$(mktemp -d)"
trap 'rm -rf "${sandbox}"' EXIT
# Keep the pinned toolchains and downloaded dependencies available after HOME changes.
export CARGO_HOME="${CARGO_HOME:-${HOME}/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-${HOME}/.rustup}"
export HOME="${sandbox}/home"
export XDG_CONFIG_HOME="${sandbox}/config"
export XDG_DATA_HOME="${sandbox}/data"
export XDG_CACHE_HOME="${sandbox}/cache"
export XDG_STATE_HOME="${sandbox}/state"
export XDG_RUNTIME_DIR="${sandbox}/runtime"
mkdir -p "${HOME}" "${XDG_CONFIG_HOME}" "${XDG_DATA_HOME}" \
  "${XDG_CACHE_HOME}" "${XDG_STATE_HOME}" "${XDG_RUNTIME_DIR}"
chmod 700 "${XDG_RUNTIME_DIR}"
unset DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE
unset DISPLAY WAYLAND_DISPLAY SESSION_MANAGER
export GDK_BACKEND=x11
# No accessibility service activation or connection to a host session.
export NO_AT_BRIDGE=1
# A normal session bus loads system service directories, which can activate a
# portal or secret provider. This bus permits explicit test-owned services only.
bus_config="${sandbox}/bus.conf"
cat > "${bus_config}" <<EOF_BUS
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=${XDG_RUNTIME_DIR}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF_BUS
xvfb-run --auto-servernum --server-args='-screen 0 1280x1024x24 -nolisten tcp' \
  dbus-run-session --config-file="${bus_config}" -- "$@"
