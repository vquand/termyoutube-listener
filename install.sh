# shellcheck shell=sh
set -eu

APP_NAME="ytmtui"
GUI_NAME="ytmtui-gui"
PREFIX="${PREFIX:-${HOME}/.local}"
DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
BIN_DIR="${PREFIX}/bin"
APP_DIR="${DATA_HOME}/applications"
ICON_ROOT="${DATA_HOME}/icons/hicolor"
ICON_DIR="${ICON_ROOT}/scalable/apps"

cd -- "$(dirname -- "$0")"

if [ "${1:-}" = "--help" ]; then
    printf 'Usage: sh install.sh\n'
    printf '\n'
    printf 'Environment overrides:\n'
    printf '  PREFIX       install prefix, defaults to $HOME/.local\n'
    printf '  XDG_DATA_HOME desktop/icon data dir, defaults to $HOME/.local/share\n'
    exit 0
fi

if ! cargo_path="$(command -v cargo 2>&1)"; then
    printf 'cargo is required to build %s, but it was not found on PATH.\n' "${APP_NAME}" >&2
    exit 1
fi

if ! install_path="$(command -v install 2>&1)"; then
    printf 'install is required, but it was not found on PATH.\n' >&2
    exit 1
fi

printf 'Building release binaries...\n'
cargo build --release --bins

mkdir -p "${BIN_DIR}" "${APP_DIR}" "${ICON_DIR}"

install_binary() {
    binary_name="$1"
    source_path="target/release/${binary_name}"
    target_path="${BIN_DIR}/${binary_name}"

    if [ -f "${target_path}" ]; then
        action="Updated"
    else
        action="Installed"
    fi

    install -m 0755 "${source_path}" "${target_path}"
    printf '%s %s at %s\n' "${action}" "${binary_name}" "${target_path}"
}

install_binary "${APP_NAME}"
install_binary "${GUI_NAME}"

install -m 0644 "packaging/linux/${APP_NAME}.svg" "${ICON_DIR}/${APP_NAME}.svg"

desktop_file="${APP_DIR}/${GUI_NAME}.desktop"
if [ -f "${desktop_file}" ]; then
    desktop_action="Updated"
else
    desktop_action="Installed"
fi

sed "s|^Exec=.*|Exec=${BIN_DIR}/${GUI_NAME}|" \
    "packaging/linux/${GUI_NAME}.desktop" > "${desktop_file}"
chmod 0644 "${desktop_file}"
printf '%s desktop launcher at %s\n' "${desktop_action}" "${desktop_file}"

if update_desktop_database="$(command -v update-desktop-database 2>&1)"; then
    update-desktop-database "${APP_DIR}" || true
fi

if gtk_update_icon_cache="$(command -v gtk-update-icon-cache 2>&1)"; then
    gtk-update-icon-cache -q "${ICON_ROOT}" || true
fi

printf 'Done.\n'
