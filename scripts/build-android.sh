#!/usr/bin/env bash
# Сборка Android TV-приложения Aurora (debug .apk или подписанный release .aab).
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
#   scripts/build-android.sh --aab                     # release для Google Play
# Переменные подписи AURORA_ANDROID_* описаны в README.md.
set -euo pipefail

FORMAT=apk
if [[ "${1:-}" == "--aab" ]]; then
  FORMAT=aab
  shift
fi
if [[ $# -gt 1 || "${1:-}" == --* ]]; then
  echo "Использование: $0 [--aab] [server-url]" >&2
  exit 1
fi
SERVER_URL="${1:-${AURORA_SERVER_URL:-}}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WEBOS="$ROOT/frontend/webos"
STAGE="$ROOT/build/android-$FORMAT"
OUT="$ROOT/build/$FORMAT"
IMAGE="aurora-android-builder"
SIGNING_ARGS=()

if [[ "$FORMAT" == aab ]]; then
  for name in AURORA_ANDROID_KEYSTORE AURORA_ANDROID_STORE_PASSWORD AURORA_ANDROID_KEY_ALIAS AURORA_ANDROID_KEY_PASSWORD; do
    if [[ -z "${!name:-}" ]]; then
      echo "Для AAB задайте $name (см. README.md)." >&2
      exit 1
    fi
  done
  if [[ ! -f "$AURORA_ANDROID_KEYSTORE" ]]; then
    echo "Keystore не найден: $AURORA_ANDROID_KEYSTORE" >&2
    exit 1
  fi
  KEYSTORE="$(cd "$(dirname "$AURORA_ANDROID_KEYSTORE")" && pwd)/$(basename "$AURORA_ANDROID_KEYSTORE")"
  SIGNING_ARGS=(
    --mount "type=bind,source=$KEYSTORE,target=/signing/upload.jks,readonly"
    -e AURORA_ANDROID_KEYSTORE=/signing/upload.jks
    -e AURORA_ANDROID_STORE_PASSWORD -e AURORA_ANDROID_KEY_ALIAS -e AURORA_ANDROID_KEY_PASSWORD
  )
fi
if [[ -n "${AURORA_ANDROID_VERSION_CODE:-}" ]] &&
   [[ ! "$AURORA_ANDROID_VERSION_CODE" =~ ^[1-9][0-9]{0,9}$ || "$AURORA_ANDROID_VERSION_CODE" -gt 2100000000 ]]; then
  echo "AURORA_ANDROID_VERSION_CODE должен быть целым числом от 1 до 2100000000." >&2
  exit 1
fi

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
echo ">> gradle ($FORMAT)"
docker run --rm \
  -v "$ROOT/android:/src:ro" \
  -v "$STAGE:/stage:ro" \
  -v "$OUT:/out" \
  -v aurora-android-gradle:/root/.gradle \
  -e VERSION="$VERSION" -e HOST_ID="$(id -u):$(id -g)" \
  -e FORMAT="$FORMAT" -e AURORA_ANDROID_VERSION_CODE \
  "${SIGNING_ARGS[@]}" \
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
    VERSION_ARGS=(-PauroraVersion="$VERSION")
    if [[ -n "${AURORA_ANDROID_VERSION_CODE:-}" ]]; then
      VERSION_ARGS+=(-PauroraVersionCode="$AURORA_ANDROID_VERSION_CODE")
    fi
    if [[ "$FORMAT" == aab ]]; then
      gradle --quiet --no-daemon bundleRelease "${VERSION_ARGS[@]}"
      ARTIFACT=app/build/outputs/bundle/release/app-release.aab
      # -strict отвергает unsigned entries и подпись чужим ключом.
      jarsigner -verify -strict -keystore "$AURORA_ANDROID_KEYSTORE" \
        -storepass:env AURORA_ANDROID_STORE_PASSWORD "$ARTIFACT" "$AURORA_ANDROID_KEY_ALIAS"
    else
      gradle --quiet --no-daemon assembleDebug "${VERSION_ARGS[@]}"
      ARTIFACT=app/build/outputs/apk/debug/app-debug.apk
    fi
    cp "$ARTIFACT" "/out/aurora-$VERSION.$FORMAT"
    chown "$HOST_ID" "/out/aurora-$VERSION.$FORMAT"
  '

echo ">> готово: $OUT/aurora-$VERSION.$FORMAT"
