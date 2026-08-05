#!/usr/bin/env bash
# Проверяет, попадают ли резы ffmpeg ровно в заданные границы сегментов при `-c:v copy`.
#
# Зачем: номер сегмента в плейлисте обязан соответствовать ровно тому отрезку, который
# в плейлисте заявлен. Муксеры ffmpeg трактуют `-segment_times` и `-ss` неочевидно
# (относительно старта рана, до/после -copyts), поэтому рецепт запуска подбирается замером.
#
# Использование:
#   scripts/hls-cut-probe.sh [файл]
# Без аргумента генерирует фикстуру с неравномерными keyframes.
set -euo pipefail
# Дробные числа везде с точкой — и в printf, и в awk
export LC_ALL=C

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

SRC=${1:-}
KEYFRAMES="0,4,9.5,14.5,17,19.5,22,25.5"
if [[ -z "$SRC" ]]; then
  SRC="$WORK/src.mkv"
  ffmpeg -y -loglevel error \
    -f lavfi -i "testsrc=duration=30:size=320x240:rate=25" \
    -f lavfi -i "sine=frequency=440:duration=30" \
    -c:v libx264 -pix_fmt yuv420p -sc_threshold 0 -force_key_frames "$KEYFRAMES" \
    -c:a ac3 -shortest "$SRC"
fi

# Границы сегментов: keyframes, схлопнутые правилом «не ближе 4 c» (как в src/segments.rs)
mapfile -t BOUNDARIES < <(
  ffprobe -v error -select_streams v:0 -show_entries packet=pts_time,flags -of csv=p=0 "$SRC" |
    awk -F, '$2 ~ /^K/ {print $1}' |
    awk 'NR==1 {last=-100} {if ($1 - last >= 4) {print $1; last=$1}}'
)
DURATION=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$SRC")
BOUNDARIES+=("$DURATION")

echo "Границы карты: ${BOUNDARIES[*]}"

# Замер одного рецепта: запускает ран с сегмента $2 и печатает реальные начала сегментов.
probe_recipe() {
  local name=$1 start_index=$2
  local dir="$WORK/$name-$start_index"
  mkdir -p "$dir"

  local start=${BOUNDARIES[$start_index]}
  # Точки реза, оставшиеся после старта рана
  local abs=() rel=()
  for ((i = start_index + 1; i < ${#BOUNDARIES[@]} - 1; i++)); do
    abs+=("${BOUNDARIES[$i]}")
    rel+=("$(awk -v a="${BOUNDARIES[$i]}" -v b="$start" 'BEGIN{printf "%.6f", a-b}')")
  done
  local abs_csv rel_csv
  abs_csv=$(
    IFS=,
    echo "${abs[*]}"
  )
  rel_csv=$(
    IFS=,
    echo "${rel[*]}"
  )

  local common=(-map 0:v:0 -map 0:a:0 -c:v copy -c:a aac -b:a 192k -ac 2
    -muxdelay 0 -muxpreload 0 -f segment -segment_format mpegts
    -segment_time_delta 0.1 -reset_timestamps 0
    -segment_start_number "$start_index" "$dir/segment%05d.ts")

  case "$name" in
  # Вход ищется по -ss (быстро, но попадает на keyframe НЕ ближе цели), точки реза абсолютные
  input-ss-abs)
    ffmpeg -y -loglevel error -ss "$start" -i "$SRC" -copyts -start_at_zero \
      -segment_times "$abs_csv" "${common[@]}" 2>/dev/null || true
    ;;
  # То же, но точки реза относительно старта рана
  input-ss-rel)
    ffmpeg -y -loglevel error -ss "$start" -i "$SRC" -copyts -start_at_zero \
      -segment_times "$rel_csv" "${common[@]}" 2>/dev/null || true
    ;;
  # Поиск на выходе: таймлайн рана начинается ровно с границы, обратно в абсолютное
  # время его возвращает -output_ts_offset. Точки реза относительные.
  output-ss-rel)
    ffmpeg -y -loglevel error -i "$SRC" -ss "$start" -copyts -output_ts_offset "$start" \
      -segment_times "$rel_csv" "${common[@]}" 2>/dev/null || true
    ;;
  # То же, но абсолютные точки реза
  output-ss-abs)
    ffmpeg -y -loglevel error -i "$SRC" -ss "$start" -copyts -output_ts_offset "$start" \
      -segment_times "$abs_csv" "${common[@]}" 2>/dev/null || true
    ;;
  # Поиск на выходе без -copyts: таймлайн с нуля, абсолютность возвращает -output_ts_offset
  output-ss-plain-rel)
    ffmpeg -y -loglevel error -i "$SRC" -ss "$start" -output_ts_offset "$start" \
      -segment_times "$rel_csv" "${common[@]}" 2>/dev/null || true
    ;;
  # Вход ищется с запасом (-ss до -i), точную границу задаёт выходной -ss (таймлайн
  # рана уезжает в 0), обратно в абсолютное время возвращает -output_ts_offset,
  # точки реза — относительные.
  hybrid-rel | hybrid-abs)
    local slack rel
    slack=$(awk -v a="$start" 'BEGIN{v=a-20; if (v<0) v=0; printf "%.6f", v}')
    rel=$(awk -v a="$start" -v b="$slack" 'BEGIN{printf "%.6f", a-b}')
    local times=$rel_csv
    [[ "$name" == hybrid-abs ]] && times=$abs_csv
    ffmpeg -y -loglevel error -ss "$slack" -i "$SRC" -ss "$rel" \
      -output_ts_offset "$start" -segment_times "$times" "${common[@]}" 2>/dev/null || true
    ;;
  # Никакого планировщика резов: на каждый сегмент свой ffmpeg с точным диапазоном.
  # Вход ищется приблизительно (-ss до -i, попадает на keyframe не дальше цели), а
  # точную границу задаёт -ss на выходе; -copyts оставляет абсолютные таймстемпы.
  per-segment | per-segment-copyts)
    local copyts=()
    [[ "$name" == per-segment-copyts ]] && copyts=(-copyts)
    for ((i = start_index; i < ${#BOUNDARIES[@]} - 1; i++)); do
      local from=${BOUNDARIES[$i]} to=${BOUNDARIES[$((i + 1))]} len slack rel
      len=$(awk -v a="$to" -v b="$from" 'BEGIN{printf "%.6f", a-b}')
      slack=$(awk -v a="$from" 'BEGIN{v=a-20; if (v<0) v=0; printf "%.6f", v}')
      # После входного -ss таймстемпы сдвинуты к нулю, поэтому выходной -ss — относительный
      rel=$(awk -v a="$from" -v b="$slack" 'BEGIN{printf "%.6f", a-b}')
      # С -copyts таймстемпы остаются исходными, значит выходной -ss абсолютный
      local out_ss=$rel offset=(-output_ts_offset "$from")
      if [[ "$name" == per-segment-copyts ]]; then
        out_ss=$from
        offset=()
      fi
      ffmpeg -y -loglevel error -ss "$slack" -i "$SRC" -ss "$out_ss" -t "$len" \
        "${copyts[@]}" "${offset[@]}" \
        -map 0:v:0 -map 0:a:0 -c:v copy -c:a aac -b:a 192k -ac 2 \
        -muxdelay 0 -muxpreload 0 -f mpegts \
        "$dir/$(printf 'segment%05d.ts' "$i")" 2>/dev/null || true
    done
    ;;
  esac

  local ok=ok
  local i=$start_index
  local report=""
  for f in "$dir"/segment*.ts; do
    [[ -e "$f" ]] || {
      report="файлов нет"
      ok=FAIL
      break
    }
    local got want
    got=$(ffprobe -v error -show_entries format=start_time -of csv=p=0 "$f")
    want=${BOUNDARIES[$i]}
    report+=$(printf " %s:%.2f/%.2f" "$(basename "$f")" "$got" "$want")
    awk -v g="$got" -v w="$want" 'BEGIN{exit (g-w < 0.3 && w-g < 0.3) ? 0 : 1}' || ok=FAIL
    i=$((i + 1))
  done
  # Не досчитались сегментов до конца файла — тоже провал
  [[ $i -eq $((${#BOUNDARIES[@]} - 1)) ]] || ok=FAIL

  printf '%-22s start=%-2s %-5s%s\n' "$name" "$start_index" "$ok" "$report"
}

for recipe in hybrid-rel hybrid-abs per-segment; do
  probe_recipe "$recipe" 0
  probe_recipe "$recipe" 3
done
