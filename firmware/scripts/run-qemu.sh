#!/usr/bin/env bash
# run-qemu.sh — Build, validate, and emulate the SandOS firmware in QEMU.
#
# Invoked automatically by `cargo run` via the [target] runner in
# firmware/.cargo/config.toml.  It can also be called directly:
#
#   ./scripts/run-qemu.sh <path-to-elf>
#
# Required tools
# ──────────────
#   espflash >= 3.0        cargo install espflash
#   qemu-system-xtensa     Espressif QEMU fork — see README §Emulation for
#                          installation instructions.
#
# Environment variables
# ──────────────────────
#   SANDOS_VALIDATE_ONLY=1   Convert ELF → BIN and verify structure, then
#                            exit without starting QEMU.  Useful in CI.

set -euo pipefail

# ── Target constants (ESP32-S3 DevKitC-1 N16R8) ───────────────────────────────
readonly CHIP="esp32s3"
readonly FLASH_SIZE="16mb"
readonly FLASH_MODE="qio"
readonly FLASH_FREQ="80m"
readonly QEMU_MACHINE="esp32s3"
readonly QEMU_BINARY="qemu-system-xtensa"
readonly ESPFLASH_MIN_MAJOR=3

# ── Colour helpers ────────────────────────────────────────────────────────────
red()    { printf '\033[0;31m%s\033[0m\n' "$*"; }
green()  { printf '\033[0;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[0;33m%s\033[0m\n' "$*"; }
info()   { printf '  \033[0;34m»\033[0m  %s\n' "$*"; }
die()    { red "ERROR: $*"; exit 1; }

# ── Argument ──────────────────────────────────────────────────────────────────
ELF="${1:?Usage: run-qemu.sh <path-to-elf>}"
[[ -f "$ELF" ]] || die "ELF not found: $ELF"

# Derive a sibling .bin path (*.bin is already in .gitignore)
BIN="${ELF%.elf}.bin"

# ── Tool checks ───────────────────────────────────────────────────────────────
check_espflash() {
    if ! command -v espflash &>/dev/null; then
        die "espflash not found.  Install it with:  cargo install espflash"
    fi

    local ver major
    ver=$(espflash --version 2>&1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
    major=$(printf '%s' "$ver" | cut -d. -f1)

    if [[ -z "$ver" ]]; then
        yellow "Warning: could not detect espflash version; proceeding anyway."
    elif [[ "$major" -lt "$ESPFLASH_MIN_MAJOR" ]]; then
        yellow "Warning: espflash $ver detected; version ${ESPFLASH_MIN_MAJOR}.x or later is recommended."
        yellow "         Upgrade with:  cargo install espflash"
    else
        info "espflash $ver — OK"
    fi
}

check_qemu() {
    if ! command -v "$QEMU_BINARY" &>/dev/null; then
        red "ERROR: $QEMU_BINARY not found."
        echo ""
        yellow "Install Espressif's QEMU fork (the upstream QEMU does not support ESP32-S3):"
        echo ""
        echo "  Option A — Pre-built release (recommended):"
        echo "    https://github.com/espressif/qemu/releases"
        echo "    Download the 'xtensa' archive for your OS, extract it, and add the"
        echo "    'bin/' directory to your PATH."
        echo ""
        echo "  Option B — Build from source:"
        echo "    git clone --depth 1 https://github.com/espressif/qemu"
        echo "    cd qemu"
        echo "    ./configure --target-list=xtensa-softmmu \\"
        echo "                --enable-gcrypt \\"
        echo "                --disable-werror"
        echo "    make -j\$(nproc)"
        echo "    sudo make install"
        echo ""
        echo "  After installation, ensure 'qemu-system-xtensa --version' works."
        echo ""
        exit 1
    fi

    local ver
    ver=$("$QEMU_BINARY" --version 2>&1 | head -1)
    info "$QEMU_BINARY — $ver"
}

# ── ELF → flash image ─────────────────────────────────────────────────────────
elf_to_bin() {
    info "Converting ELF → flash image …"
    info "  ELF : $ELF"
    info "  BIN : $BIN"

    espflash save-image \
        --chip       "$CHIP"       \
        --flash-size "$FLASH_SIZE" \
        --flash-mode "$FLASH_MODE" \
        --flash-freq "$FLASH_FREQ" \
        "$ELF" "$BIN"

    [[ -f "$BIN" ]] || die "espflash did not produce: $BIN"

    local size
    size=$(du -sh "$BIN" | cut -f1)
    green "  Flash image generated: $BIN  ($size)"
}

# ── QEMU boot ─────────────────────────────────────────────────────────────────
boot_qemu() {
    green ""
    green "Booting SandOS in QEMU (machine: $QEMU_MACHINE) …"
    echo "  Serial output is wired to this terminal."
    echo "  Press Ctrl-A then X to exit QEMU."
    echo ""

    # Boot mode strapping: strap_mode=0x02 sets GPIO0=1 (high), which selects
    # SPI flash boot mode on the ESP32-S3.  Without this the chip would enter
    # UART download mode and sit waiting for firmware upload instead of booting.
    exec "$QEMU_BINARY"                                          \
        -nographic                                               \
        -machine "$QEMU_MACHINE"                                 \
        -drive   "file=${BIN},if=mtd,format=raw"                \
        -global  "driver=esp32s3.gpio,property=strap_mode,value=0x02"
}

# ── Main ──────────────────────────────────────────────────────────────────────
echo ""
green "── SandOS QEMU Runner ────────────────────────────────────────────────"
info  "Target : $CHIP  (flash $FLASH_SIZE $FLASH_MODE @ $FLASH_FREQ)"
info  "ELF    : $ELF"
echo ""

check_espflash
check_qemu

elf_to_bin

if [[ "${SANDOS_VALIDATE_ONLY:-0}" == "1" ]]; then
    green ""
    green "Validation complete (SANDOS_VALIDATE_ONLY=1 — skipping QEMU)."
    exit 0
fi

boot_qemu
