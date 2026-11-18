#!/usr/bin/env bash
set -euo pipefail

OLLAMA_URL="http://localhost:11434"
MODEL="qwen3:0.6b"
PID_FILE="${TMPDIR:-/tmp}/axon-ollama.pid"
LOG_FILE="${TMPDIR:-/tmp}/axon-ollama.log"

start_server() {
    if ! curl -sf "${OLLAMA_URL}/api/version" &>/dev/null; then
        echo "Starting ollama serve..." >&2
        nohup ollama serve >>"${LOG_FILE}" 2>&1 &
        echo $! > "${PID_FILE}"
        local retries=30
        while (( retries > 0 )); do
            if curl -sf "${OLLAMA_URL}/api/version" &>/dev/null; then
                break
            fi
            sleep 1
            retries=$((retries - 1))
        done
        if (( retries == 0 )); then
            echo "ERROR: ollama did not become ready within 30s" >&2
            exit 1
        fi
        echo "ollama is running." >&2
    else
        echo "ollama already running." >&2
    fi
}

cmd_up() {
    if ! command -v ollama &>/dev/null; then
        echo "ollama not found, installing via brew..." >&2
        brew install ollama >&2
    fi
    start_server

    echo "Pulling ${MODEL}..." >&2
    ollama pull "${MODEL}" >&2

    echo "Loading ${MODEL}..." >&2
    curl -sf -X POST "${OLLAMA_URL}/api/generate" \
        -H "Content-Type: application/json" \
        -d "{\"model\":\"${MODEL}\",\"prompt\":\"hi\",\"stream\":false}" >/dev/null

    echo "BASE_URL=${OLLAMA_URL}/v1"
    echo "MODEL=${MODEL}"
}

cmd_down() {
    echo "Unloading ${MODEL}..." >&2
    curl -sf -X POST "${OLLAMA_URL}/api/generate" \
        -H "Content-Type: application/json" \
        -d "{\"model\":\"${MODEL}\",\"keep_alive\":0}" >/dev/null || true

    if [ -f "${PID_FILE}" ]; then
        local pid
        pid=$(cat "${PID_FILE}")
        echo "Stopping ollama (pid ${pid})..." >&2
        kill "${pid}" 2>/dev/null || true
        rm -f "${PID_FILE}"
    fi
}

cmd_purge() {
    start_server
    echo "Removing ${MODEL}..." >&2
    ollama rm "${MODEL}"
    cmd_down
}

case "${1:-}" in
    up)    cmd_up ;;
    down)  cmd_down ;;
    purge) cmd_purge ;;
    *)     echo "Usage: $0 {up|down|purge}" >&2; exit 1 ;;
esac
