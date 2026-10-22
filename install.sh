#!/usr/bin/env bash
# Install crab from a release build.
#
# Usage:
#   ./install.sh                     install under /usr/local (may need sudo)
#   PREFIX="$HOME/.local" ./install.sh
#   DESTDIR=/tmp/pkgroot ./install.sh  stage a package install
#   ./install.sh --no-build          install the existing target/release binaries
#
# The default build is the nested winit backend. For a bare-metal session, build
# with the TTY feature first and then install without rebuilding:
#   CARGO_FEATURES=backend-tty ./install.sh
#
# `CARGO_FEATURES` is passed to the crab binary build only; the CLI and IPC
# crates are built with their defaults.
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PREFIX="${PREFIX:-/usr/local}"
DESTDIR="${DESTDIR:-}"
BUILD=1
INSTALL_SESSION=1

usage() {
    cat <<'EOF'
Install crab.

Options:
  --prefix <dir>     installation prefix (default: /usr/local)
  --destdir <dir>    staging root prepended to every path (default: empty)
  --no-build         use the binaries already in target/release
  --no-session       do not install the desktop entry and systemd user units
  -h, --help         show this help

Environment:
  PREFIX             same as --prefix
  DESTDIR            same as --destdir
  CARGO_FEATURES     extra cargo features for the crab binary (for example
                     backend-tty)
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix)
            [ "$#" -ge 2 ] || { echo "install.sh: --prefix needs a value" >&2; exit 2; }
            PREFIX="$2"
            shift 2
            ;;
        --destdir)
            [ "$#" -ge 2 ] || { echo "install.sh: --destdir needs a value" >&2; exit 2; }
            DESTDIR="$2"
            shift 2
            ;;
        --no-build)
            BUILD=0
            shift
            ;;
        --no-session)
            INSTALL_SESSION=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "install.sh: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case "$PREFIX" in
    /*) ;;
    *)
        echo "install.sh: PREFIX must be an absolute path (got '$PREFIX')" >&2
        exit 2
        ;;
esac
case "$DESTDIR" in
    ""|/*) ;;
    *)
        echo "install.sh: DESTDIR must be empty or absolute (got '$DESTDIR')" >&2
        exit 2
        ;;
esac

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN_DIR="$DESTDIR$PREFIX/bin"
DOC_DIR="$DESTDIR$PREFIX/share/doc/crab"
SYSTEMD_DIR="$DESTDIR$PREFIX/lib/systemd/user"
WAYLAND_SESSION_DIR="$DESTDIR$PREFIX/share/wayland-sessions"
PORTAL_DIR="$DESTDIR$PREFIX/share/xdg-desktop-portal"

if [ "$BUILD" -eq 1 ]; then
    echo "building crab (release)"
    if [ -n "${CARGO_FEATURES:-}" ]; then
        cargo build --release --manifest-path "$ROOT/Cargo.toml" -p crab \
            --features "$CARGO_FEATURES"
    else
        cargo build --release --manifest-path "$ROOT/Cargo.toml" -p crab
    fi
    cargo build --release --manifest-path "$ROOT/Cargo.toml" -p crab-cli
fi

install -Dm755 "$TARGET_DIR/release/crab" "$BIN_DIR/crab"
if [ -x "$TARGET_DIR/release/crab-cli" ]; then
    install -Dm755 "$TARGET_DIR/release/crab-cli" "$BIN_DIR/crab-cli"
fi

install -Dm644 "$ROOT/resources/default-config.toml" \
    "$DOC_DIR/default-config.toml"
install -Dm644 "$ROOT/ROADMAP.md" "$DOC_DIR/ROADMAP.md"
install -Dm644 "$ROOT/resources/crab-portals.conf" \
    "$PORTAL_DIR/crab-portals.conf"

if [ "$INSTALL_SESSION" -eq 1 ]; then
    # The shipped units assume /usr. Rewrite their absolute paths so a
    # non-standard prefix still produces a working session.
    tmp="$(mktemp)"
    trap 'rm -f "$tmp"' EXIT
    sed \
        -e "s|/usr/bin/crab|$PREFIX/bin/crab|g" \
        -e "s|/usr/share/doc/crab|$PREFIX/share/doc/crab|g" \
        "$ROOT/resources/session/crab.service" >"$tmp"
    install -Dm644 "$tmp" "$SYSTEMD_DIR/crab.service"
    sed "s|/usr/share/doc/crab|$PREFIX/share/doc/crab|g" \
        "$ROOT/resources/session/crab-session.target" >"$tmp"
    install -Dm644 "$tmp" "$SYSTEMD_DIR/crab-session.target"
    rm -f "$tmp"
    trap - EXIT

    sed "s|^Exec=crab$|Exec=$PREFIX/bin/crab|" \
        "$ROOT/resources/session/crab.desktop" |
        install -Dm644 /dev/stdin "$WAYLAND_SESSION_DIR/crab.desktop"
fi

cat <<EOF
installed:
  $BIN_DIR/crab
EOF
if [ -x "$BIN_DIR/crab-cli" ]; then
    echo "  $BIN_DIR/crab-cli"
fi
cat <<EOF
  $DOC_DIR/default-config.toml
  $DOC_DIR/ROADMAP.md
  $PORTAL_DIR/crab-portals.conf
EOF
if [ "$INSTALL_SESSION" -eq 1 ]; then
    cat <<EOF
  $SYSTEMD_DIR/crab.service
  $SYSTEMD_DIR/crab-session.target
  $WAYLAND_SESSION_DIR/crab.desktop
EOF
fi
