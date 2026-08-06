# Tech Stack

## Бэкенд
- Rust, `edition = "2024"`, единый бинарь `aurora` (плоский набор модулей в `src/`, без workspace).
- axum 0.7 + tower-http 0.5 (cors/trace/fs), tokio (full).
- sqlx 0.7, SQLite (`runtime-tokio-rustls`, `chrono`, `migrate`). Миграции — `migrations/*.sql`, применяются на старте.
- jsonwebtoken 9 + bcrypt 0.15 — авторизация; reqwest 0.12 (rustls, без default-features) — TMDB.
- rust-embed 8 — фронтенд-бандл внутри бинаря; sysinfo 0.30 — метрики.
- Внешние бинарники: `ffmpeg`, `ffprobe` обязаны быть в PATH.

## Фронтенд (`frontend/`)
- Svelte 5 (runes) + TypeScript + Vite 5. Ни SvelteKit, ни CSS-фреймворка.
- axios, hls.js 1.6.
- svelte-check + stylelint 17 (`stylelint-config-standard`, `postcss-html`) — оба входят в `npm run build`.
- npm (package-lock.json). Есть корневой `package.json` — вспомогательный, основной — `frontend/package.json`.

## Инфраструктура
- Taskfile (`task`) как основной раннер, docker-compose + Dockerfile (всё на `:4000`).
- webOS-приложение собирается из того же фронта через `scripts/build-webos.sh` (ares-* CLI).
