#!/usr/bin/env bash
# Проверка промотки транскодированного потока: дёргает stream-transcoded с разными
# оффсетами и показывает тайм-стемпы из ffprobe. Помогает увидеть, где seek «уезжает»,
# особенно на границе 120-сек чанка (TRANSCODE_CHUNK_DURATION в src/server.rs).
#
# Использование: scripts/seek-check.sh <media_id> [base_url]
set -euo pipefail

MEDIA_ID="${1:?укажите media_id}"
BASE_URL="${2:-http://localhost:4000}"
OFFSETS=(0 30 90 125)

printf '%-8s %-12s %-12s\n' "start" "duration" "start_time"
for start in "${OFFSETS[@]}"; do
  url="${BASE_URL}/api/media/${MEDIA_ID}/stream-transcoded?start=${start}"
  probe=$(curl -s "$url" \
    | ffprobe -v error -show_entries format=duration,start_time \
        -of default=noprint_wrappers=1:nokey=1 - 2>/dev/null || true)
  duration=$(sed -n '1p' <<<"$probe")
  start_time=$(sed -n '2p' <<<"$probe")
  printf '%-8s %-12s %-12s\n' "$start" "${duration:-ERR}" "${start_time:-ERR}"
done
