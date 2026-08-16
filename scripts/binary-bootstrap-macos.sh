#!/usr/bin/env bash
# binary-bootstrap-macos.sh
# Downloads and installs the pinned llama-server binary for macOS Apple Silicon.

set -euo pipefail

LLAMA_VERSION="b10448"
PLATFORM="macos-arm64"
ARCHIVE_NAME="llama-${LLAMA_VERSION}-bin-${PLATFORM}.tar.gz"
DOWNLOAD_URL="https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_VERSION}/${ARCHIVE_NAME}"
EXPECTED_SHA256="4ba9efd97b9915a16906fff0e8addfc514575a1a34e1b30dafd8424e2b12d927"

INSTALL_DIR="${HOME}/.llm-toolkit/bin"
VERSION_LOCK="${INSTALL_DIR}/version.lock"
BINARY_PATH="${INSTALL_DIR}/llama-server"

# --- Check if already installed ---
if [[ -f "${VERSION_LOCK}" && -f "${BINARY_PATH}" ]]; then
    installed=$(cat "${VERSION_LOCK}" | tr -d '[:space:]')
    if [[ "${installed}" == "${LLAMA_VERSION}" ]]; then
        echo "[OK] llama-server ${LLAMA_VERSION} already installed at ${INSTALL_DIR}"
        exit 0
    fi
    echo "[INFO] Installed version (${installed}) differs from pinned (${LLAMA_VERSION}). Updating."
fi

mkdir -p "${INSTALL_DIR}"

# --- Download ---
TEMP_DIR=$(mktemp -d)
ARCHIVE_PATH="${TEMP_DIR}/${ARCHIVE_NAME}"

echo "[INFO] Downloading ${ARCHIVE_NAME}..."
curl -fSL "${DOWNLOAD_URL}" -o "${ARCHIVE_PATH}"
echo "[OK] Download complete."

# --- Tier 1: SHA256 ---
echo "[INFO] Verifying SHA256..."
actual=$(shasum -a 256 "${ARCHIVE_PATH}" | awk '{print $1}')
if [[ "${actual}" != "${EXPECTED_SHA256}" ]]; then
    echo "[FAIL] SHA256 mismatch. Expected: ${EXPECTED_SHA256} Got: ${actual}"
    exit 1
fi
echo "[OK] SHA256 verified."

# --- Tier 3: Archive size ---
archive_size=$(stat -f%z "${ARCHIVE_PATH}")
if [[ ${archive_size} -lt 5242880 ]]; then
    echo "[FAIL] Archive suspiciously small (${archive_size} bytes)."
    exit 1
fi
echo "[OK] Archive size check passed ($(( archive_size / 1048576 )) MB)."

# --- Extract ---
echo "[INFO] Extracting to ${INSTALL_DIR}..."
tar -xzf "${ARCHIVE_PATH}" -C "${INSTALL_DIR}"
chmod +x "${BINARY_PATH}"

# --- Tier 3: Binary size ---
binary_size=$(stat -f%z "${BINARY_PATH}")
if [[ ${binary_size} -lt 102400 ]]; then
    echo "[FAIL] llama-server binary suspiciously small."
    exit 1
fi
echo "[OK] Binary size check passed."

# --- Version lock ---
echo "${LLAMA_VERSION}" > "${VERSION_LOCK}"
echo "[OK] Version lock written: ${LLAMA_VERSION}"

rm -rf "${TEMP_DIR}"
echo "[OK] Temp files removed."
echo ""
echo "llama-server ${LLAMA_VERSION} installed at ${INSTALL_DIR}"