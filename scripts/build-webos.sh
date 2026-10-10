#!/usr/bin/env bash
# Сборка webOS-приложения Aurora (.ipk).
#
# Приложение — тонкая оболочка (frontend/webos/index.html): хранит адрес сервера,
# проверяет его доступность и переходит на SPA, которую отдаёт сам сервер.
# Поэтому фронтенд здесь не собирается, а обновление сервера обновляет и ТВ.
# Адрес вводится в приложении при первом запуске; опционально его можно задать
# по умолчанию первым аргументом/env AURORA_SERVER_URL.
#
# Требования: node, ares-cli (npm i -g @webosose/ares-cli).
#
# Использование:
#   scripts/build-webos.sh                            # адрес вводится в приложении
#   scripts/build-webos.sh http://192.168.1.10:4000   # адрес по умолчанию
set -euo pipefail

SERVER_URL="${1:-${AURORA_SERVER_URL:-}}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WEBOS="$ROOT/frontend/webos"
STAGE="$ROOT/build/webos"
OUT="$ROOT/build/ipk"

# Версия приложения = version из package.json (единый источник)
VERSION="$(node -p "require('$ROOT/frontend/package.json').version")"

echo ">> подготовка staging: $STAGE (version $VERSION)"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$WEBOS/icon.png" "$WEBOS/largeIcon.png" "$STAGE/"
if [[ -n "$SERVER_URL" ]]; then
  echo ">> адрес сервера по умолчанию: $SERVER_URL"
  sed "s|__AURORA_SERVER_URL__|${SERVER_URL//|/\\|}|g" "$WEBOS/index.html" > "$STAGE/index.html"
else
  cp "$WEBOS/index.html" "$STAGE/"
fi
# Синхронизируем version в appinfo.json из package.json
node -e "const a=require('$WEBOS/appinfo.json'); a.version='$VERSION'; require('fs').writeFileSync('$STAGE/appinfo.json', JSON.stringify(a,null,2)+'\n')"

echo ">> ares-package -> $OUT"
mkdir -p "$OUT"
ares-package -n "$STAGE" -o "$OUT"

echo ">> готово. .ipk в $OUT:"
ls -1 "$OUT"/*.ipk
