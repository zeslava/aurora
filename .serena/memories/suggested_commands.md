# Команды

Основной раннер — `task` (Taskfile.yml, подхватывает `.env`). `task --list` — весь список.

## Разработка
- `task dev` — бэкенд (`cargo run`, порт 4000, БД `./data/aurora.db`) + Vite параллельно.
- `task backend` / `task frontend` — по отдельности.
- `task install` — `npm install` во `frontend/`.

## Сборка
- `task build` — фронт (`npm run build` → `frontend/dist`) + `cargo build --release`.
- `task run` — запуск релизного бинаря (создаёт `data/`).
- `task docker-up` / `task docker-down` — та же БД через bind-mount `./data`.

## Тесты
- `task test` → `cargo test` (включает матрицу форм источника, нужен ffmpeg, ~10 с).
- `task test-unit` — быстрые, без ffmpeg: `cargo test --bin aurora -- parse_range range_start_seconds needs_video_transcoding`.
- `task test-integration` — `cargo test --bin aurora -- --nocapture direct_stream transcoded`.
- `cargo test real_library_conformance -- --ignored` — прогон по реальной библиотеке из `data/aurora.db`; см. `mem:backend/streaming`.

## Линт
- `task lint` = `cargo clippy` + `cargo fmt --check`; `task fmt` — форматирование.
- `npm run lint:css` во `frontend/` — stylelint.

## webOS
`task webos-build` → `webos-install` → `webos-launch` (или `webos-deploy` разом). Устройство — `WEBOS_DEVICE` (default `LGTV`), app id `com.aurora.mediaserver`. `task webos-inspect` — DevTools приложения на телевизоре (консоль плеера, Network).

## Диагностика
`scripts/hls-cut-probe.sh`, `scripts/seek-check.sh` — ручная проверка нарезки и промотки.
