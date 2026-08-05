#!/usr/bin/env bash
# Сборка Aurora в webOS-приложение (.ipk).
#
# Приложение — статичный SPA, который обращается к серверу Aurora по HTTP.
# Адрес сервера по умолчанию вводится в самом приложении при первом запуске
# (экран ServerSetup) и хранится в localStorage — один .ipk работает с любым
# сервером. Опционально адрес можно зашить в билд через первый аргумент/env
# (VITE_API_URL) — тогда экран ввода не показывается.
#
# Требования: node/npm, ares-cli (npm i -g @webosose/ares-cli).
#
# Использование:
#   scripts/build-webos.sh                       # адрес вводится в приложении
#   scripts/build-webos.sh http://192.168.1.10:4000   # адрес зашит в билд
set -euo pipefail

SERVER_URL="${1:-${AURORA_SERVER_URL:-}}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FRONTEND="$ROOT/frontend"
STAGE="$ROOT/build/webos"
OUT="$ROOT/build/ipk"

# Версия приложения = version из package.json (единый источник)
VERSION="$(node -p "require('$FRONTEND/package.json').version")"

if [[ -n "$SERVER_URL" ]]; then
  echo ">> сборка фронтенда (адрес сервера зашит: $SERVER_URL)"
else
  echo ">> сборка фронтенда (адрес сервера вводится в приложении)"
fi
cd "$FRONTEND"
npm ci
VITE_API_URL="$SERVER_URL" VITE_BASE="./" npm run build

echo ">> подготовка staging: $STAGE (version $VERSION)"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -r "$FRONTEND/dist/." "$STAGE/"
cp "$FRONTEND/webos/icon.png" "$FRONTEND/webos/largeIcon.png" "$STAGE/"
# Синхронизируем version в appinfo.json из package.json
node -e "const a=require('$FRONTEND/webos/appinfo.json'); a.version='$VERSION'; require('fs').writeFileSync('$STAGE/appinfo.json', JSON.stringify(a,null,2)+'\n')"

echo ">> ares-package -> $OUT"
mkdir -p "$OUT"
# -n: без встроенной минификации (uglify-js в ares понимает только ES5, а vite уже минифицировал)
ares-package -n "$STAGE" -o "$OUT"

echo ">> готово. .ipk в $OUT:"
ls -1 "$OUT"/*.ipk
