# Фронтенд — карта (`frontend/`)

Vite + Svelte 5 (runes) + TS, обычный SPA-бандл. Прод отдаёт `frontend/dist` (вшит в бинарь) — пересобирать после правок.

- `src/api/client.ts` (axios) — общение с бэком. На 401 вызывает обработчик, зарегистрированный через `onUnauthorized`, **не делает жёсткий редирект** (сломал бы hash-роутинг в упакованном webOS-приложении).
- `src/lib/router.svelte.ts` — самописный роутер (~90 строк): history-режим в вебе, hash-режим при `apiClient.isPackagedApp()`. Экспортирует `router`, `navigate()`, действие `link`.
- `src/lib/auth.svelte.ts` — rune-стор с состоянием JWT.
- `src/lib/spatialNavigation.ts`, `src/lib/pointerMode.ts`, `src/lib/tvMode.ts` — D-pad телевизора и Magic Remote; `init*()` возвращают cleanup, вызываются из `$effect` в `App.svelte`.
- `src/routes/` — `Login`, `Library`, `MovieDetail`, `SeriesDetail`, `Watch`, `ServerSetup`. Параметры маршрута приходят пропсами.
- `src/components/` — переиспользуемая разметка; логика плеера — `VideoPlayer.svelte` (hls.js).
- webOS: тот же бандл упаковывается в `.ipk` через `scripts/build-webos.sh`; отладка на телевизоре — `task webos-inspect`.

Стили, токены и правила линтера — `mem:frontend/design_system` (читать до добавления любого стиля).
