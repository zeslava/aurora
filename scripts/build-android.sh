#!/usr/bin/env bash
# Сборка Android TV-приложения Aurora (.apk).
#
# Приложение — WebView-оболочка над той же страницей, что и webOS-.ipk
# (frontend/webos/index.html): хранит адрес сервера, проверяет его доступность и
# переходит на SPA, которую отдаёт сам сервер. Фронтенд здесь не собирается.
# Адрес вводится в приложении при первом запуске; опционально его можно задать
# по умолчанию первым аргументом/env AURORA_SERVER_URL.
#
# Требования: node, docker. Android SDK и Gradle живут в образе (android/Dockerfile),
# кэш Gradle — в docker volume aurora-android-gradle.
#
# Использование:
#   scripts/build-android.sh                            # адрес вводится в приложении
#   scripts/build-android.sh http://192.168.1.10:4000   # адрес по умолчанию
set -euo pipefail

SERVER_URL="${1:-${AURORA_SERVER_URL:-}}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WEBOS="$ROOT/frontend/webos"
STAGE="$ROOT/build/android"
OUT="$ROOT/build/apk"
IMAGE="aurora-android-builder"

# Версия приложения = version из package.json (единый источник)
VERSION="$(node -p "require('$ROOT/frontend/package.json').version")"

echo ">> подготовка staging: $STAGE (version $VERSION)"
rm -rf "$STAGE"
mkdir -p "$STAGE" "$OUT"
cp "$WEBOS/largeIcon.png" "$STAGE/"
if [[ -n "$SERVER_URL" ]]; then
  echo ">> адрес сервера по умолчанию: $SERVER_URL"
  sed "s|__AURORA_SERVER_URL__|${SERVER_URL//|/\\|}|g" "$WEBOS/index.html" > "$STAGE/index.html"
else
  cp "$WEBOS/index.html" "$STAGE/"
fi

echo ">> образ сборки: $IMAGE"
docker build -q -t "$IMAGE" "$ROOT/android" > /dev/null

# Проект копируется внутрь контейнера: build/ и .gradle/ от root не попадают в репозиторий
echo ">> gradle assembleDebug"
docker run --rm \
  -v "$ROOT/android:/src:ro" \
  -v "$STAGE:/stage:ro" \
  -v "$OUT:/out" \
  -v aurora-android-gradle:/root/.gradle \
  -e VERSION="$VERSION" -e HOST_ID="$(id -u):$(id -g)" \
  "$IMAGE" bash -euo pipefail -c '
    cp -r /src /work && cd /work/app/src/main
    mkdir -p assets
    cp /stage/index.html assets/
    for d in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:144; do
      mkdir -p "res/mipmap-${d%%:*}"
      convert /stage/largeIcon.png -resize "${d##*:}x${d##*:}" "res/mipmap-${d%%:*}/ic_launcher.png"
    done
    mkdir -p res/drawable
    convert -size 320x180 xc:"#0f172a" \
      \( /stage/largeIcon.png -resize 110x110 \) -gravity west -geometry +24+0 -composite \
      -fill "#e2e8f0" -font DejaVu-Sans-Bold -pointsize 40 -gravity west -annotate +150+0 Aurora \
      res/drawable/banner.png
    cd /work
    gradle --quiet --no-daemon assembleDebug -PauroraVersion="$VERSION"
    cp app/build/outputs/apk/debug/app-debug.apk "/out/aurora-$VERSION.apk"
    chown "$HOST_ID" "/out/aurora-$VERSION.apk"
  '

echo ">> готово. .apk в $OUT:"
ls -1 "$OUT"/*.apk
