#!/usr/bin/env bash
# ==============================================================================
#  ⚡ unisoc-tool-rs Universal One-Line Installer (Linux, macOS, Android/Termux)
#  https://github.com/Milordick/unisoc-tool-rs
# ==============================================================================

set -euo pipefail

REPO="Milordick/unisoc-tool-rs"
GITHUB_URL="https://github.com/${REPO}"
RAW_URL="https://raw.githubusercontent.com/${REPO}/main"

# ANSI Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

echo -e "${CYAN}${BOLD}"
echo "  ⚡ ========================================================= ⚡"
echo "               UNISOC-TOOL-RS INSTALLER / UPDATER               "
echo "       Cross-Platform Spreadtrum / Unisoc Service Tool          "
echo "  ⚡ ========================================================= ⚡"
echo -e "${NC}"

# 1. Detect Operating System
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
IS_TERMUX=false

if [[ -d "/data/data/com.termux/files/usr" ]] || [[ "${PREFIX:-}" == *"/com.termux/"* ]]; then
    IS_TERMUX=true
    OS="android"
fi

case "${OS}" in
    linux)   TARGET_OS="linux" ;;
    darwin)  TARGET_OS="macos" ;;
    android) TARGET_OS="linux" ;;
    *)
        echo -e "${RED}[ERROR] Неподдерживаемая ОС: ${OS}.${NC}"
        echo "Для Windows используйте PowerShell: irm ${RAW_URL}/install.ps1 | iex"
        exit 1
        ;;
esac

# 2. Detect Architecture
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64|amd64)
        TARGET_ARCH="x86_64"
        ;;
    aarch64|arm64)
        TARGET_ARCH="aarch64"
        ;;
    armv7l|armv7|armhf)
        TARGET_ARCH="armv7"
        ;;
    *)
        echo -e "${RED}[ERROR] Неизвестная архитектура: ${ARCH}.${NC}"
        exit 1
        ;;
esac

echo -e "${BLUE}[INFO] Обнаружена система:${NC} ${BOLD}${TARGET_OS} (${TARGET_ARCH})${NC}"
if [ "${IS_TERMUX}" = true ]; then
    echo -e "${YELLOW}[NOTE] Запуск внутри Termux на Android. Для работы с USB понадобятся root-права (tsu).${NC}"
fi

# 3. Determine install directories
if [ "${IS_TERMUX}" = true ]; then
    INSTALL_DIR="${PREFIX}/opt/unisoc-tool"
    BIN_DIR="${PREFIX}/bin"
elif [ "$(id -u)" -eq 0 ]; then
    INSTALL_DIR="/opt/unisoc-tool"
    BIN_DIR="/usr/local/bin"
else
    INSTALL_DIR="${HOME}/.local/share/unisoc-tool"
    BIN_DIR="${HOME}/.local/bin"
fi

mkdir -p "${INSTALL_DIR}" "${BIN_DIR}"

# 4. Get latest release tag from GitHub API
echo -e "${BLUE}[INFO] Поиск последнего релиза на GitHub...${NC}"
LATEST_TAG=""
RELEASE_JSON=""
if command -v curl >/dev/null 2>&1; then
    RELEASE_JSON="$(curl -fsSL -H "User-Agent: unisoc-installer" "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null || true)"
fi

if [[ -n "${RELEASE_JSON}" ]] && [[ "${RELEASE_JSON}" == *"tag_name"* ]]; then
    LATEST_TAG="$(echo "${RELEASE_JSON}" | grep -o '"tag_name": *"[^"]*"' | head -n 1 | cut -d '"' -f 4)"
fi

ASSET_NAME="unisoc-tool-${TARGET_OS}-${TARGET_ARCH}.tar.gz"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

DOWNLOAD_SUCCESS=false

if [[ -n "${LATEST_TAG}" ]]; then
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"
    echo -e "${BLUE}[INFO] Скачивание релиза ${BOLD}${LATEST_TAG}${NC} (${ASSET_NAME})...${NC}"
    if curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/bundle.tar.gz" 2>/dev/null; then
        DOWNLOAD_SUCCESS=true
    fi
fi

# Fallback: if release asset is not yet available, build from source using cargo
if [ "${DOWNLOAD_SUCCESS}" = false ]; then
    echo -e "${YELLOW}[WARN] Готовый бинарный архив релиза не найден (репозиторий новый или сборка в процессе).${NC}"
    if command -v cargo >/dev/null 2>&1; then
        echo -e "${BLUE}[INFO] Обнаружен Cargo! Запуск быстрой сборки из исходников...${NC}"
        cd "${TMP_DIR}"
        git clone --depth 1 "${GITHUB_URL}.git" unisoc-src
        cd unisoc-src
        cargo build --release
        cp -f target/release/unisoc "${INSTALL_DIR}/unisoc"
        cp -r loaders "${INSTALL_DIR}/"
        DOWNLOAD_SUCCESS=true
    else
        echo -e "${RED}[ERROR] Не удалось скачать релиз и не установлен Cargo.${NC}"
        echo "Установите Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        echo "Или скачайте релиз вручную: ${GITHUB_URL}/releases"
        exit 1
    fi
else
    echo -e "${BLUE}[INFO] Распаковка файлов в ${INSTALL_DIR}...${NC}"
    tar -xzf "${TMP_DIR}/bundle.tar.gz" -C "${INSTALL_DIR}"
fi

# Set executable permissions
chmod +x "${INSTALL_DIR}/unisoc" 2>/dev/null || true
if [[ -f "${INSTALL_DIR}/loaders/spd_dump" ]]; then
    chmod +x "${INSTALL_DIR}/loaders/spd_dump" 2>/dev/null || true
fi

# Create symlink in bin directory
ln -sf "${INSTALL_DIR}/unisoc" "${BIN_DIR}/unisoc"

# 5. Linux udev rules installation
if [[ "${TARGET_OS}" == "linux" ]] && [[ "${IS_TERMUX}" == false ]]; then
    if [ "$(id -u)" -eq 0 ]; then
        echo -e "${BLUE}[INFO] Установка udev-правил для Unisoc (VID 1782)...${NC}"
        "${INSTALL_DIR}/unisoc" setup-linux >/dev/null 2>&1 || true
    elif sudo -n true 2>/dev/null; then
        sudo "${INSTALL_DIR}/unisoc" setup-linux >/dev/null 2>&1 || true
    fi
fi

# 6. Check PATH
PATH_OK=false
case ":$PATH:" in
    *":${BIN_DIR}:"*) PATH_OK=true ;;
esac

echo ""
echo -e "${GREEN}${BOLD}🎉 Установка успешно завершена!${NC}"
echo -e "${CYAN}------------------------------------------------------------${NC}"
echo -e "Путь к бинарнику:   ${BOLD}${INSTALL_DIR}/unisoc${NC}"
echo -e "Симлинк в PATH:     ${BOLD}${BIN_DIR}/unisoc${NC}"
echo -e "Файлы загрузчиков:  ${BOLD}${INSTALL_DIR}/loaders/${NC}"
echo -e "${CYAN}------------------------------------------------------------${NC}"

if [ "${PATH_OK}" = false ]; then
    echo -e "${YELLOW}[ВНИМАНИЕ] Каталог ${BIN_DIR} отсутствует в вашем PATH!${NC}"
    echo "Добавьте его, выполнив команду:"
    echo -e "  ${BOLD}export PATH=\"${BIN_DIR}:\$PATH\"${NC}"
    echo "и пропишите эту строку в свой ~/.bashrc или ~/.zshrc."
    echo ""
fi

echo -e "Для запуска графического интерфейса введите:"
echo -e "  ${GREEN}${BOLD}unisoc${NC}"
echo ""
echo -e "Для проверки обновлений:"
echo -e "  ${BLUE}unisoc update${NC}"
echo ""
