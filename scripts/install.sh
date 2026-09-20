#!/bin/sh
set -eu

REPO="BoringInfraCo/Root"
VERSION=""
DRY_RUN=false
ASSUME_YES=false
BINARY_NAME="root"
INSTALL_DIR="/usr/local/bin"

usage() {
    cat <<EOF
Usage: $(basename "$0") [--version VERSION] [--dry-run]

Install Root — a deterministic package manager for developer CLI tools.

Root requires Nix. If Nix is not found, the script offers to install it
using the official Determinate Systems installer.

Options:
  --version VERSION   Install a specific version (default: latest)
  --dry-run           Print what would be done without doing it
  --yes               Skip the Nix installation confirmation prompt
  -h, --help          Show this help message
EOF
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            if [ -z "${2:-}" ]; then
                echo "Error: --version requires a version argument." >&2
                echo "Usage: $(basename "$0") [--version VERSION] [--dry-run] [--yes]" >&2
                exit 1
            fi
            VERSION="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --yes)
            ASSUME_YES=true
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            echo "Error: Unknown option: $1" >&2
            echo "Usage: $(basename "$0") [--version VERSION] [--dry-run] [--yes]" >&2
            exit 1
            ;;
    esac
done

detect_os_arch() {
    case "$(uname -s)" in
        Darwin) OS="apple-darwin" ;;
        Linux)  OS="unknown-linux-gnu" ;;
        *)
            echo "Error: Unsupported operating system: $(uname -s)" >&2
            exit 1
            ;;
    esac

    case "$(uname -m)" in
        arm64|aarch64) ARCH="aarch64" ;;
        x86_64|amd64)  ARCH="x86_64" ;;
        *)
            echo "Error: Unsupported architecture: $(uname -m)" >&2
            exit 1
            ;;
    esac
}

fetch_latest_version() {
    url="https://api.github.com/repos/${REPO}/releases/latest"
    fetch_cmd=""

    if command -v curl >/dev/null 2>&1; then
        fetch_cmd="curl -sL"
    elif command -v wget >/dev/null 2>&1; then
        fetch_cmd="wget -qO-"
    else
        echo "Error: Neither curl nor wget found. Please install one of them." >&2
        exit 1
    fi

    $fetch_cmd "$url" | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p'
}

download_archive() {
    url="$1"
    output="$2"

    if command -v curl >/dev/null 2>&1; then
        curl -sL -o "$output" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$output" "$url"
    else
        echo "Error: Neither curl nor wget found. Please install one of them." >&2
        exit 1
    fi
}

# ------------------------------------------------------------------
# Nix detection and installation
# ------------------------------------------------------------------

ensure_nix() {
    if command -v nix >/dev/null 2>&1; then
        return 0
    fi

    echo ""
    echo "Root requires Nix."
    echo ""
    echo "Nix was not found on this machine."
    echo ""
    echo "Root can install Nix for you using the official Determinate Systems installer."
    echo "This may modify your shell profile and create /nix."
    echo ""

    if [ "$ASSUME_YES" = true ]; then
        install_nix
        return $?
    fi

    # Prompt on /dev/tty so it works with curl | sh
    printf "Continue? [y/N] " > /dev/tty 2>/dev/null || true
    read -r answer < /dev/tty 2>/dev/null || answer="n"

    case "$answer" in
        [yY]|[yY][eE][sS])
            install_nix
            ;;
        *)
            echo ""
            echo "Installation cancelled."
            echo ""
            echo "Install Nix first, then rerun the Root installer."
            exit 0
            ;;
    esac
}

install_nix() {
    echo ""
    echo "Installing Nix..."
    echo "Note: The Nix installer is downloaded from install.determinate.systems."
    echo "Root does not verify the Nix installer checksum as Determinate Systems"
    echo "does not currently publish a separate checksum for the installer script."
    echo "The download uses HTTPS with TLS 1.2+ for transport security."
    echo ""

    if ! command -v curl >/dev/null 2>&1; then
        echo "Error: curl is required to install Nix." >&2
        exit 1
    fi

    NIX_INSTALLER_URL="https://install.determinate.systems/nix"
    NIX_INSTALLER_TMP="${ROOT_INSTALL_TMPDIR:-/tmp}/nix-installer.sh"

    echo "Downloading Nix installer..."
    if ! curl --proto '=https' --tlsv1.2 -sSf -L -o "$NIX_INSTALLER_TMP" "$NIX_INSTALLER_URL"; then
        echo "Error: Failed to download Nix installer." >&2
        exit 1
    fi

    if [ ! -s "$NIX_INSTALLER_TMP" ]; then
        echo "Error: Downloaded Nix installer is empty." >&2
        exit 1
    fi

    echo "Running Nix installer..."
    if ! sh "$NIX_INSTALLER_TMP" install; then
        echo "Error: Nix installation failed." >&2
        exit 1
    fi

    echo "Nix installed successfully."
    echo ""

    if [ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]; then
        . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
    elif [ -f "$HOME/.nix-profile/etc/profile.d/nix.sh" ]; then
        . "$HOME/.nix-profile/etc/profile.d/nix.sh"
    fi

    if command -v nix >/dev/null 2>&1; then
        return 0
    fi

    echo "Nix was installed but is not available in the current shell."
    echo "Please restart your shell and rerun the Root installer."
    exit 1
}

# ------------------------------------------------------------------
# Root download, verification, and installation
# ------------------------------------------------------------------

install_root() {
    detect_os_arch

    if [ -z "$VERSION" ]; then
        echo "Fetching latest version..."
        VERSION=$(fetch_latest_version)
        if [ -z "$VERSION" ]; then
            echo "Error: Could not determine the latest version." >&2
            exit 1
        fi
        echo "Latest version: v${VERSION}"
    fi

    FILENAME="root-${ARCH}-${OS}.tar.gz"
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${FILENAME}"
    CHECKSUM_URL="https://github.com/${REPO}/releases/download/v${VERSION}/checksums.txt"

    ROOT_INSTALL_TMPDIR=$(mktemp -d 2>/dev/null || mktemp -d -t 'root-install')
    trap 'rm -rf "${ROOT_INSTALL_TMPDIR:-}"' EXIT

    echo "Downloading ${FILENAME}..."
    download_archive "$DOWNLOAD_URL" "${ROOT_INSTALL_TMPDIR}/${FILENAME}"

    if [ ! -f "${ROOT_INSTALL_TMPDIR}/${FILENAME}" ] || [ ! -s "${ROOT_INSTALL_TMPDIR}/${FILENAME}" ]; then
        echo "Error: Failed to download ${DOWNLOAD_URL}" >&2
        exit 1
    fi

    echo "Downloading checksums..."
    CHECKSUMS_FILE="${ROOT_INSTALL_TMPDIR}/checksums.txt"
    download_archive "$CHECKSUM_URL" "$CHECKSUMS_FILE"

    if [ ! -f "$CHECKSUMS_FILE" ] || [ ! -s "$CHECKSUMS_FILE" ]; then
        echo "Error: Failed to download checksums file from ${CHECKSUM_URL}" >&2
        exit 1
    fi

    EXPECTED_SHA=$(grep -F "${FILENAME}" "$CHECKSUMS_FILE" | head -n1 | awk '{print $1}')
    if [ -z "$EXPECTED_SHA" ]; then
        echo "Error: Could not find checksum entry for ${FILENAME} in checksums file." >&2
        exit 1
    fi

    SHA_CMD=""
    if command -v sha256sum >/dev/null 2>&1; then
        SHA_CMD="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        SHA_CMD="shasum -a 256"
    else
        echo "Error: No SHA-256 utility found (sha256sum or shasum required)." >&2
        exit 1
    fi

    COMPUTED_SHA=$($SHA_CMD < "${ROOT_INSTALL_TMPDIR}/${FILENAME}" | awk '{print $1}')
    COMPUTED_SHA=$(echo "$COMPUTED_SHA" | tr -d ' ')
    if [ "$COMPUTED_SHA" != "$EXPECTED_SHA" ]; then
        echo "Error: SHA256 mismatch!" >&2
        echo "  Expected: ${EXPECTED_SHA}" >&2
        echo "  Got:      ${COMPUTED_SHA}" >&2
        exit 1
    fi
    echo "Checksum verified successfully."

    echo "Extracting..."
    cd "$ROOT_INSTALL_TMPDIR"
    if command -v tar >/dev/null 2>&1; then
        tar xzf "${FILENAME}"
    else
        echo "Error: tar is required to extract the archive." >&2
        exit 1
    fi

    if [ ! -f "$BINARY_NAME" ]; then
        echo "Error: Binary '$BINARY_NAME' not found in archive." >&2
        exit 1
    fi

    SUDO=""
    if [ ! -w "$INSTALL_DIR" ]; then
        if command -v sudo >/dev/null 2>&1; then
            echo "No write permission to ${INSTALL_DIR}. Using sudo..."
            SUDO="sudo"
        else
            echo "Error: No write permission to ${INSTALL_DIR} and sudo not found." >&2
            echo "Please run: curl -fsSL https://raw.githubusercontent.com/${REPO}/main/scripts/install.sh | sudo sh" >&2
            exit 1
        fi
    fi

    $SUDO cp "$BINARY_NAME" "${INSTALL_DIR}/${BINARY_NAME}"
    $SUDO chmod 755 "${INSTALL_DIR}/${BINARY_NAME}"
    echo "Root installed successfully."
    echo ""
}

# ------------------------------------------------------------------
# Verification
# ------------------------------------------------------------------

run_doctor() {
    echo "Verifying installation..."

    DOCTOR_CMD="${INSTALL_DIR}/${BINARY_NAME}"
    if [ -x "$DOCTOR_CMD" ]; then
        if "$DOCTOR_CMD" doctor >/dev/null 2>&1; then
            echo "Root is ready."
        else
            echo "Error: 'root doctor' reported issues. Run 'root doctor' for details." >&2
            exit 1
        fi
    elif command -v root >/dev/null 2>&1; then
        if root doctor >/dev/null 2>&1; then
            echo "Root is ready."
        else
            echo "Error: 'root doctor' reported issues. Run 'root doctor' for details." >&2
            exit 1
        fi
    else
        echo "Error: Root binary not found in PATH after install." >&2
        echo "Ensure ${INSTALL_DIR} is in your PATH, then run 'root doctor'." >&2
        exit 1
    fi
}

# ------------------------------------------------------------------
# Main
# ------------------------------------------------------------------

detect_os_arch

if [ "$DRY_RUN" = true ]; then
    echo "=== Dry Run ==="
    echo "Repository:     ${REPO}"
    echo "Version:        v${VERSION:-latest}"
    echo "OS/Arch:        ${ARCH}-${OS}"
    echo "Install path:   ${INSTALL_DIR}/${BINARY_NAME}"
    echo "================"
    exit 0
fi

ensure_nix
install_root
run_doctor
