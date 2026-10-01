#!/usr/bin/env bash
set -euo pipefail

# Default install directory
DEFAULT_DIR="${HOME}/.local/share/antigravity-acp"
TARGET_DIR="${1:-$DEFAULT_DIR}"

# Detect OS
OS="$(uname -s)"
case "${OS}" in
  Darwin*)  PLATFORM="macos" ;;
  Linux*)   PLATFORM="linux" ;;
  *)        echo "Unsupported OS: ${OS}"; exit 1 ;;
esac

# Detect Architecture
ARCH="$(uname -m)"
case "${ARCH}" in
  arm64|aarch64) ARCH_TAG="arm64" ;;
  x86_64|amd64)  ARCH_TAG="x86_64" ;;
  *)             echo "Unsupported architecture: ${ARCH}"; exit 1 ;;
esac

# Resolve official Google ACP release asset URL
if [[ "${PLATFORM}" == "macos" && "${ARCH_TAG}" == "arm64" ]]; then
  DOWNLOAD_URL="https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-agy_acp_server_1.1.1-darwin-arm64.zip"
elif [[ "${PLATFORM}" == "linux" && "${ARCH_TAG}" == "x86_64" ]]; then
  DOWNLOAD_URL="https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-agy_acp_server_1.1.1-linux-x86_64.zip"
elif [[ "${PLATFORM}" == "linux" && "${ARCH_TAG}" == "arm64" ]]; then
  DOWNLOAD_URL="https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-agy_acp_server_1.1.1-linux-arm64.zip"
else
  echo "No official Google Antigravity ACP release found for ${PLATFORM}-${ARCH_TAG}"
  exit 1
fi

echo "=================================================="
echo "Installing Google Antigravity ACP Server"
echo "Platform:    ${PLATFORM}-${ARCH_TAG}"
echo "Target Dir:  ${TARGET_DIR}"
echo "URL:         ${DOWNLOAD_URL}"
echo "=================================================="

mkdir -p "${TARGET_DIR}"

TMP_ZIP="$(mktemp -t agy_acp_download.XXXXXX.zip)"
trap 'rm -f "${TMP_ZIP}"' EXIT

echo "Downloading release archive from dl.google.com..."
curl -fSL --progress-bar "${DOWNLOAD_URL}" -o "${TMP_ZIP}"

echo "Extracting runtime binaries..."
unzip -q -o "${TMP_ZIP}" -d "${TARGET_DIR}"

if [[ -f "${TARGET_DIR}/agy_acp_server.par" ]]; then
  chmod +x "${TARGET_DIR}/agy_acp_server.par"
fi

if [[ -f "${TARGET_DIR}/localharness_external" ]]; then
  chmod +x "${TARGET_DIR}/localharness_external"
fi

echo "=================================================="
echo "Successfully installed Antigravity ACP Server!"
echo "Server binary:  ${TARGET_DIR}/agy_acp_server.par"
if [[ -f "${TARGET_DIR}/localharness_external" ]]; then
  echo "Harness binary: ${TARGET_DIR}/localharness_external"
fi
echo ""
echo "To use this server with meta-harness, run:"
echo "  export AGY_EXECUTABLE=\"${TARGET_DIR}/agy_acp_server.par\""
echo "=================================================="
