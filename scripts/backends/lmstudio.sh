#!/usr/bin/env bash
set -euo pipefail

BASE_URL="http://localhost:1234/v1"
MODEL="axon-test"
STARTED_FILE="${TMPDIR:-/tmp}/axon-lmstudio.started"
MODEL_DIR="$HOME/.lmstudio/models/lmstudio-community/Qwen3-0.6B-MLX-4bit"

cmd_up() {
    lms get qwen/qwen3-0.6b -y >&2

    if ! curl -sf http://localhost:1234/v1/models >/dev/null 2>&1; then
        lms server start >&2
        echo "started" > "${STARTED_FILE}"
    fi

    if ! lms ps | grep -q "axon-test"; then
        lms load qwen/qwen3-0.6b --identifier axon-test -y >&2
    fi

    echo "BASE_URL=${BASE_URL}"
    echo "MODEL=${MODEL}"
}

cmd_down() {
    if lms ps | grep -q "axon-test"; then
        lms unload axon-test >&2
    fi

    if [ -f "${STARTED_FILE}" ]; then
        lms server stop >&2
        rm -f "${STARTED_FILE}"
    fi
}

cmd_purge() {
    cmd_down
    if [ -d "$MODEL_DIR" ]; then
        rm -rf "$MODEL_DIR"
        echo "Removed: ${MODEL_DIR}" >&2
    fi
}

case "${1:-}" in
    up)    cmd_up ;;
    down)  cmd_down ;;
    purge) cmd_purge ;;
    *)     echo "Usage: $0 {up|down|purge}" >&2; exit 1 ;;
esac
