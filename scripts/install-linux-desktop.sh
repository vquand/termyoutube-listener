#!/usr/bin/env sh
set -eu

APP_ID="ytmtui-gui"
ICON_NAME="ytmtui"
ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
BIN_DIR="${HOME}/.local/bin"
APP_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/applications"
ICON_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/icons/hicolor/scalable/apps"

if ! command -v cargo >/dev/null 2>&1; then
    printf 'cargo is required to build ytmtui-gui, but it was not found on PATH.\n' >&2
    exit 1
fi

cargo build --release --bin "${APP_ID}"

mkdir -p "${BIN_DIR}" "${APP_DIR}" "${ICON_DIR}"
install -m 0755 "${ROOT_DIR}/target/release/${APP_ID}" "${BIN_DIR}/${APP_ID}"
install -m 0644 "${ROOT_DIR}/packaging/linux/ytmtui.svg" "${ICON_DIR}/${ICON_NAME}.svg"

desktop_file="${APP_DIR}/${APP_ID}.desktop"
sed "s|^Exec=.*|Exec=${BIN_DIR}/${APP_ID}|" \
    "${ROOT_DIR}/packaging/linux/${APP_ID}.desktop" > "${desktop_file}"
chmod 0644 "${desktop_file}"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${APP_DIR}" >/dev/null 2>&1 || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q "${XDG_DATA_HOME:-${HOME}/.local/share}/icons/hicolor" >/dev/null 2>&1 || true
fi

printf 'Installed %s for this user.\n' "${APP_ID}"
printf 'Launcher: %s\n' "${desktop_file}"
printf 'If GNOME does not show it immediately, log out and back in or run: gtk-launch %s\n' "${APP_ID}"
