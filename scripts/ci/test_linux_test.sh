#!/usr/bin/env bash
# Check isolation without starting a display, D-Bus, native app or keyring.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "${fixture}"' EXIT
mkdir "${fixture}/bin"
cat > "${fixture}/bin/xvfb-run" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == --auto-servernum && "$2" == --server-args=* ]]
shift 2
exec "$@"
STUB
cat > "${fixture}/bin/dbus-run-session" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == --config-file=* ]]
config="${1#--config-file=}"
[[ -f "${config}" ]]
# No includes or service directories may import host activation definitions.
! grep -Eq '<(include|includedir|servicedir|standard_session_servicedirs|standard_system_servicedirs)([[:space:]/>])' "${config}"
grep -q '<type>session</type>' "${config}"
grep -q "<listen>unix:tmpdir=${XDG_RUNTIME_DIR}</listen>" "${config}"
grep -q '<allow own="\*"/>' "${config}"
grep -q '<allow send_destination="\*"/>' "${config}"
grep -q '<allow receive_sender="\*"/>' "${config}"
shift
[[ "$1" == -- ]]
shift
exec "$@"
STUB
chmod +x "${fixture}/bin/"*
export PATH="${fixture}/bin:${PATH}"
export DBUS_SESSION_BUS_ADDRESS=host DBUS_STARTER_ADDRESS=host DBUS_STARTER_BUS_TYPE=session
export DISPLAY=:host WAYLAND_DISPLAY=host SESSION_MANAGER=host
export CARGO_HOME="${fixture}/cargo" RUSTUP_HOME="${fixture}/rustup"
"${root}/scripts/ci/linux-test.sh" bash -c '
  set -euo pipefail
  [[ "$CARGO_HOME" == "$1/cargo" && "$RUSTUP_HOME" == "$1/rustup" ]]
  [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}${DBUS_STARTER_ADDRESS:-}${DBUS_STARTER_BUS_TYPE:-}" ]]
  [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}${SESSION_MANAGER:-}" ]]
  [[ "$GDK_BACKEND" == x11 && "$NO_AT_BRIDGE" == 1 ]]
  for directory in "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"; do
    [[ -d "$directory" && "$directory" != "$1"/* ]]
  done
  [[ "$(stat -c %a "$XDG_RUNTIME_DIR")" == 700 ]]
  printf "%s\n" "$HOME" > "$1/home-path"
' bash "${fixture}"
[[ ! -e "$(cat "${fixture}/home-path")" ]]
