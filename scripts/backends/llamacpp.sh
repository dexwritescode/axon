#!/usr/bin/env bash
set -euo pipefail

BASE_URL="http://localhost:8080"
MODEL="qwen3-0.6b"
PID_FILE="${TMPDIR:-/tmp}/axon-llamacpp.pid"
LOG_FILE="${TMPDIR:-/tmp}/axon-llamacpp.log"

cmd_up() {
    if ! command -v llama-server &>/dev/null; then
        echo "llama-server not found, installing via brew..." >&2
        brew install llama.cpp >&2
    fi

    if ! curl -sf "${BASE_URL}/health" &>/dev/null; then
        echo "Starting llama-server..." >&2
        nohup llama-server -hf Qwen/Qwen3-0.6B-GGUF:Q8_0 --alias qwen3-0.6b --port 8080 >>"${LOG_FILE}" 2>&1 &
        echo $! > "${PID_FILE}"
        local retries=300
        while (( retries > 0 )); do
            if curl -sf "${BASE_URL}/health" &>/dev/null; then
                break
            fi
            sleep 1
            retries=$((retries - 1))
        done
        if (( retries == 0 )); then
            echo "ERROR: llama-server did not become ready within 300s" >&2
            tail -20 "${LOG_FILE}" >&2
            exit 1
        fi
        echo "llama-server is running." >&2
    else
        echo "llama-server already running." >&2
    fi

    echo "BASE_URL=${BASE_URL}/v1"
    echo "MODEL=${MODEL}"
}

cmd_down() {
    if [ -f "${PID_FILE}" ]; then
        local pid
        pid=$(cat "${PID_FILE}")
        echo "Stopping llama-server (pid ${pid})..." >&2
        kill "${pid}" 2>/dev/null || true
        rm -f "${PID_FILE}"
    fi
}

cmd_purge() {
    cmd_down
    local model_dir="$HOME/.cache/huggingface/hub/models--Qwen--Qwen3-0.6B-GGUF"
    local lock_dir="$HOME/.cache/huggingface/hub/.locks/models--Qwen--Qwen3-0.6B-GGUF"
    local removed=false

    if [ -d "$model_dir" ]; then
        rm -rf "$model_dir"
        echo "Removed: ${model_dir}" >&2
        removed=true
    fi

    if [ -d "$lock_dir" ]; then
        rm -rf "$lock_dir"
        echo "Removed: ${lock_dir}" >&2
        removed=true
    fi

    if [ "$removed" = false ]; then
        echo "Nothing to remove." >&2
    fi
}

case "${1:-}" in
    up)    cmd_up ;;
    down)  cmd_down ;;
    purge) cmd_purge ;;
    *)     echo "Usage: $0 {up|down|purge}" >&2; exit 1 ;;
esac
