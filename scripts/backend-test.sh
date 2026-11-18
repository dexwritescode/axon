#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
dir="$repo/scripts/backends"

if [ -n "${1:-}" ] && [ -x "$dir/$1.sh" ]; then
    NAME="$1"
else
    echo "Usage: $0 NAME" >&2
    ls "$dir" | sed 's/\.sh$//' | paste -sd ' ' - >&2
    exit 1
fi

trap "\"$dir/$NAME.sh\" down" EXIT

OUTPUT=$("$dir/$NAME.sh" up)
BASE_URL=$(echo "$OUTPUT" | grep '^BASE_URL=' | head -1 | cut -d= -f2-)
MODEL=$(echo "$OUTPUT" | grep '^MODEL=' | head -1 | cut -d= -f2-)

cd "$repo"
AXON_LIVE_BACKEND="$NAME" AXON_LIVE_BASE_URL="$BASE_URL" AXON_LIVE_MODEL="$MODEL" cargo test -p axon-core live -- --ignored --nocapture
