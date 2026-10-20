# 0009 — Android TV-приложение — та же оболочка в WebView

Дата: 2026-10-17.

## Контекст

Нужен клиент для Android TV. Варианты: нативный UI (Compose for TV + ExoPlayer) или
WebView. Нативный UI дублирует фронтенд и расходится с сервером по версиям — ровно то, от
чего ушли в 0008.

## Решение

1. **`.apk` — WebView над той же оболочкой, что и `.ipk`** (`frontend/webos/index.html`).
   `scripts/build-android.sh` кладёт её в `assets/` (с подстановкой `AURORA_SERVER_URL`),
   `MainActivity` открывает `file:///android_asset/index.html`, дальше — SPA с сервера.
   Отдельной копии страницы в `android/` нет.
2. **Маркер `AuroraTV` в User-Agent.** По нему `lib/tvMode.ts` включает ТВ-режим и
   фиксирует `meta viewport` = `width=1920`; WebView для этого держит `useWideViewPort`.
   Маркер ставится только при `UI_MODE_TYPE_TELEVISION`: тот же `.apk` на телефоне и
   планшете — обычная мобильная SPA (ориентация свободная, системные панели видны; экран
   не гаснет и уходит в landscape только в полноэкранном видео).
3. **Back пульта обрабатывает Activity:** выход из полноэкранного видео → `goBack()` по
   истории WebView (= истории SPA) → выход из приложения.
4. **Сборка в Docker** (`android/Dockerfile`: JDK 17, SDK, Gradle), локально нужен только
   `adb`. Проект копируется внутрь контейнера, в репозиторий не попадают `build/` от root.
   Иконки и баннер генерируются из `frontend/webos/largeIcon.png`.

Плеер — тот же hls.js. Аппаратный AC3/DTS через ExoPlayer сознательно не взят: потребовал
бы второй ветки плеера в `VideoPlayer.svelte` (см. 0006).

## Что сломается, если сделать «очевидным» способом

- **Определять ТВ по ширине экрана.** CSS-ширина WebView на 1080p-ТВ — 960px (density 2):
  медиа-запрос `min-width: 1900px` не срабатывает, ТВ-режим не включается.
- **Не фиксировать viewport 1920 (или убрать `useWideViewPort`).** Корневые 22px на 960px
  дают вдвое крупный UI, а брейкпоинты срабатывают как на планшете.
- **Держать копию `index.html` в `android/`.** Исправление оболочки дойдёт только до одной
  платформы.
- **Убрать `usesCleartextTraffic`.** Сервер в LAN работает по http — с `targetSdk` ≥ 28
  WebView молча не загрузит ни `/health`, ни SPA.
- **Убрать `onShowCustomView`.** `requestFullscreen` у видео перестанет работать.
- **Убрать `allowFileAccess`.** С API 30 он по умолчанию выключен — оболочка из `assets/`
  не откроется.
