#!/usr/bin/env bash
# run.sh — SandOS firmware emulation runner
#
# Cargo runner for `xtensa-esp32s3-none-elf` targets.
# Invoked automatically by `cargo run` inside the `firmware/` directory.
#
# Pipeline:
#   ELF  ──espflash save-image──►  flash.bin  ──qemu-system-xtensa──►  boot
#
# Required tools
#   • espflash  ≥ 3.0   — cargo install espflash
#   • qemu-system-xtensa (Espressif fork) — see README for install instructions
#
# Usage (direct):  ./run.sh <path/to/firmware.elf>
# Usage (cargo):   cargo run [--release]

set -euo pipefail

# ── Args ──────────────────────────────────────────────────────────────────────

ELF="${1:?ERROR: no ELF path supplied.  Usage: run.sh <path-to-elf>}"

# ── Tool version checks ───────────────────────────────────────────────────────

require_tool() {
    local cmd="$1"
    local hint="$2"
    if ! command -v "$cmd" &>/dev/null; then
        echo "ERROR: required tool '$cmd' not found in PATH."
        echo "  $hint"
        exit 1
    fi
}

require_tool espflash \
    "Install with: cargo install espflash  (https://github.com/esp-rs/espflash)"

require_tool qemu-system-xtensa \
    "Install Espressif's QEMU fork: https://github.com/espressif/qemu/releases"

ESPFLASH_VER=$(espflash --version 2>&1 | head -1)
QEMU_VER=$(qemu-system-xtensa --version 2>&1 | head -1)

echo "==> espflash : $ESPFLASH_VER"
echo "==> QEMU     : $QEMU_VER"

# Warn if espflash major version is not 3.x (match case-insensitively).
ESPFLASH_VER_LOWER="${ESPFLASH_VER,,}"
if [[ "$ESPFLASH_VER_LOWER" =~ espflash[[:space:]]([0-9]+)\. ]]; then
    ESPFLASH_MAJOR="${BASH_REMATCH[1]}"
    if [[ "$ESPFLASH_MAJOR" -lt 3 ]]; then
        echo "WARNING: espflash $ESPFLASH_VER detected; version 3.x or newer is recommended."
    fi
fi

# ── Step 1: ELF → flash image ─────────────────────────────────────────────────

FLASH_IMAGE="$(mktemp "${TMPDIR:-/tmp}/sandos_flash_XXXXXX.bin")"
# Clean up the temp image on exit (normal or error).
trap 'rm -f "$FLASH_IMAGE"' EXIT

echo "==> Converting ELF to flash image…"
echo "    ELF   : $ELF"
echo "    Image : $FLASH_IMAGE"

espflash save-image \
    --chip esp32s3 \
    --flash-size 16mb \
    --flash-mode qio \
    --flash-freq 80mhz \
    "$ELF" \
    "$FLASH_IMAGE"

IMAGE_SIZE=$(du -sh "$FLASH_IMAGE" | cut -f1)
echo "==> Flash image generated (${IMAGE_SIZE})."

# ── Step 2: Boot in QEMU ──────────────────────────────────────────────────────

echo ""
echo "==> Booting SandOS in QEMU (ESP32-S3)…"
echo "    Press  Ctrl+A  then  X  to exit QEMU."
echo ""

exec qemu-system-xtensa \
    -nographic \
    -machine esp32s3 \
    -drive file="$FLASH_IMAGE",if=mtd,format=raw \
    -serial mon:stdio
