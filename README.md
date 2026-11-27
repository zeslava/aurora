# Aurora

Self-hosted медиасервер для фильмов и сериалов: Rust/Axum-бэкенд и Svelte 5 SPA,
плюс приложения для LG webOS и Android TV. Указываете папку с файлами — Aurora сама
разбирает её на фильмы и сериалы, подтягивает обложки с TMDB и отдаёт видео в
браузер или на телевизор через HLS.

## Возможности

- **Библиотека без ручной разметки.** Фильм или сериал определяется по именам
  файлов (`SxxEyy`, `part N`), а не по структуре папок — достаточно одного корневого
  каталога.
- **Метаданные и обложки.** ffprobe — длительность, разрешение, кодеки, аудиодорожки;
  TMDB — постеры и фоны (опционально, по API-ключу).
- **Воспроизведение через HLS.** Видео копируется без перекодирования, где это
  возможно; аудио AC3/DTS/TrueHD/E-AC3 перекодируется в AAC на лету. Чересстрочные
  источники перекодируются с деинтерлейсингом.
- **Выбор аудиодорожки** при воспроизведении.
- **Продолжить просмотр.** Позиция сохраняется на сервере, следующий эпизод
  запускается автоматически.
- **Телевизор.** Управление с D-pad пульта и Magic Remote, приложения для webOS
  (`.ipk`) и Android TV (`.apk`), вход на ТВ по коду, подтверждённому с телефона.
- **Дома — без логина.** Из локальной сети всё доступно без входа, снаружи нужен
  пароль. `STRICT_AUTH=true` требует вход всегда.
- **Метрики** процесса и системы на `/api/metrics`.

## Быстрый старт (Docker)

1. Укажите в `docker-compose.yml` путь к медиатеке на хосте:
   ```yaml
   volumes:
     - ./data:/app/data
     - /path/to/media:/app/media/movies:ro
   ```
2. По желанию создайте `.env` с `TMDB_API_KEY` (см. `.env.example`).
3. Запустите:
   ```bash
   docker compose up -d --build
   ```
4. Откройте `http://localhost:4000`, создайте пользователя и запустите
   сканирование. Дальше из домашней сети вход не нужен.

БД и кэш обложек лежат в `./data` на хосте.

## Запуск без Docker

Нужны Rust (stable), Node.js и `ffmpeg`/`ffprobe` в `PATH`.

```bash
cp .env.example .env          # и отредактировать
cd frontend && npm install && npm run build && cd ..
cargo build --release
./target/release/aurora
```

Собранный фронтенд вшивается в бинарь, отдельно раздавать статику не нужно.

Если установлен [Task](https://taskfile.dev), то же самое короче: `task install`,
`task build`, `task run`. `task` без аргументов покажет все задачи.

## FreeBSD (DAIL)

После сборки фронтенда и `target/release/aurora` на FreeBSD замените
`/path/to/media` и `/path/to/aurora-data` на свои каталоги медиатеки и данных:

```bash
doas dail run aurora.dail --env-file .env \
  --mount /path/to/media:/media \
  --mount /path/to/aurora-data:/var/db/aurora \
  --network inherit --rebuild
```

### Отдельный демосервер

Используется тот же `aurora.dail`: имя jail, порт и каталоги задаются при запуске.
Замените `/path/to/aurora-demo` на свой каталог демосервера, создайте подкаталоги
и положите в `media` один-два тестовых ролика:

```bash
mkdir -p /path/to/aurora-demo/data /path/to/aurora-demo/media

doas dail run aurora.dail --name aurora-demo --env-file .env \
  -e PORT=4001 \
  -e DATABASE_URL=sqlite:/var/db/aurora/aurora.db \
  -e MEDIA_PATHS=/media \
  -e CACHE_DIR=/var/db/aurora/cache \
  -e STRICT_AUTH=true \
  -e JWT_SECRET= \
  --mount /path/to/aurora-demo/media:/media:ro \
  --mount /path/to/aurora-demo/data:/var/db/aurora \
  --network inherit --rebuild
```

`-e` переопределяет значения из `.env` и Dailfile. Пустой `JWT_SECRET` создаёт
отдельный ключ в каталоге данных демо. При `--network inherit` сервис слушает порт
из `PORT`; проброс через PF не используется. `--name aurora-demo` направляет
`--rebuild` на демо-jail; его БД и медиа лежат в отдельных каталогах.

Перед открытием внешнего доступа зайдите на `http://<адрес-сервера>:4001`, создайте
первого пользователя и обновите медиатеку. Для своего домена настройте reverse
proxy на этот адрес со схемой `http`, HTTPS-сертификат и редирект HTTP → HTTPS.
Обязательная авторизация включена и для запросов из LAN и прокси.

## Конфигурация

Переменные окружения (читаются и из `.env`):

| Переменная | По умолчанию | Описание |
|---|---|---|
| `PORT` | `4000` | Порт HTTP-сервера |
| `DATABASE_URL` / `DB_PATH` | `sqlite:./aurora.db` | Путь к SQLite |
| `MEDIA_PATHS` | — | Каталоги медиатеки через запятую |
| `JWT_SECRET` | генерируется | Ключ подписи токенов. Если не задан — создаётся и хранится в файле `jwt_secret` рядом с БД |
| `STRICT_AUTH` | `false` | `false` — из локальной сети всё доступно без входа, снаружи нужен пароль. `true` — пароль всегда. Обязательно `true` за reverse proxy на том же хосте: там все клиенты выглядят как `127.0.0.1` |
| `TMDB_API_KEY` | — | Ключ TMDB v3; без него вместо обложек — сгенерированные заглушки |
| `TMDB_LANGUAGE` | `ru-RU` | Язык метаданных TMDB |
| `CACHE_DIR` | `./cache` | Кэш обложек (`<CACHE_DIR>/images`) |
| `FFMPEG_MAX_RUNS` | `4` | Сколько ffmpeg-ранов HLS идёт одновременно; сверх лимита новый просмотр глушит раны файла, который дольше всех не запрашивали |
| `VAAPI_DEVICE` | `/dev/dri/renderD128` | Видео перекодирует iGPU через VAAPI (1080p, HDR → SDR), если пробное кодирование на старте прошло; `off` — только CPU (libx264) |
| `LOG_LEVEL` | `info` | Уровень логирования |
| `STATIC_DIR` | — | Для разработки: читать статику с диска вместо вшитой в бинарь |

«Локальная сеть» — это приватные адреса (`10.x`, `172.16–31.x`, `192.168.x`), loopback и
link-local. Если перед Aurora стоит прокси или туннель (reverse proxy, Tailscale,
rootless Docker), сервер видит его адрес, а не адрес клиента. Проверьте, кем приходят
внешние клиенты: закройте доступ снаружи на прокси или включите `STRICT_AUTH=true`.

## webOS-приложение (LG TV)

Aurora упаковывается в `.ipk`: статичный SPA, который обращается к серверу по HTTP.
Файлы приложения — в `frontend/webos/`, версия берётся из `frontend/package.json`.

Адрес сервера вводится в приложении при первом запуске (хранится в `localStorage`),
поэтому один `.ipk` работает с любым сервером. Его можно и зашить в билд —
тогда экран ввода не показывается.

### Сборка

Нужен `ares-cli` (`npm i -g @webosose/ares-cli`).

```bash
scripts/build-webos.sh                            # адрес вводится в приложении
scripts/build-webos.sh http://192.168.1.10:4000   # или зашить в билд
# результат: build/ipk/com.aurora.mediaserver_<version>_all.ipk
```

### Установка на ТВ (Developer Mode)

Разово на ТВ:
1. Установить **Developer Mode** из LG Content Store и войти аккаунтом
   [developer.lge.com](https://developer.lge.com).
2. Включить Dev Mode и Key Server — ТВ покажет IP и passphrase.

Разово на машине сборки:
```bash
ares-setup-device                          # добавить ТВ: имя, IP, порт 9922
ares-novacom --device <name> --getkey      # ввести passphrase с ТВ
```

Каждый релиз:
```bash
task webos-deploy WEBOS_DEVICE=<name>      # сборка + установка + запуск
```

> Dev Mode-сессия живёт ~50 часов, затем приложение отключается — продлевается
> в приложении Developer Mode на ТВ.

## Android TV-приложение

`.apk` — та же оболочка, что и для webOS (`frontend/webos/index.html`) в WebView:
адрес сервера вводится при первом запуске, интерфейс приходит с сервера, поэтому
обновление сервера обновляет и ТВ. Проект — в `android/`, версия берётся из
`frontend/package.json`.

Нужны `docker` и `adb`; Android SDK и Gradle живут в образе сборки (первая сборка
скачивает ~1 ГБ).

```bash
scripts/build-android.sh                            # адрес вводится в приложении
scripts/build-android.sh http://192.168.1.10:4000   # или зашить в билд
# результат: build/apk/aurora-<version>.apk (debug-подпись, для sideload)
```

На ТВ: «Настройки → Об устройстве → 7 раз по номеру сборки», затем в «Для
разработчиков» включить отладку по USB/сети.

```bash
adb connect <ip-тв>:5555                            # подтвердить на ТВ
task android-deploy ANDROID_DEVICE=<ip-тв>:5555     # сборка + установка + запуск
```

Отладка: `chrome://inspect` на ПК показывает WebView приложения (консоль, Network).

### Подписанный AAB для Google Play

Для внутреннего тестирования и публикации собирается release `.aab` с upload-ключом.
Play App Signing подписывает устанавливаемые APK своим ключом. Оболочка сохраняет
`applicationId`: `ru.zeslava.aurora`; `compileSdk` и `targetSdk` — 36.

Разово создайте upload-ключ (нужен `keytool` из JDK). Команда запросит пароль
хранилища, сведения сертификата и пароль ключа:

```bash
mkdir -p .android-signing
chmod 700 .android-signing
keytool -genkeypair -keystore .android-signing/upload.jks -storetype JKS \
  -alias aurora-upload -keyalg RSA -keysize 2048 -validity 10000
chmod 600 .android-signing/upload.jks
```

Перед сборкой задайте путь к ключу, alias и пароли. `read` не сохраняет пароль
в истории команд. Если пароли хранилища и ключа разные, запросите второй отдельно.

```bash
export AURORA_ANDROID_KEYSTORE="$PWD/.android-signing/upload.jks"
export AURORA_ANDROID_KEY_ALIAS=aurora-upload
printf 'Пароль хранилища: '
read -rs AURORA_ANDROID_STORE_PASSWORD
printf '\n'
export AURORA_ANDROID_STORE_PASSWORD
export AURORA_ANDROID_KEY_PASSWORD="$AURORA_ANDROID_STORE_PASSWORD"

scripts/build-android.sh --aab
# или: task android-aab
# с адресом демо по умолчанию:
scripts/build-android.sh --aab https://aurora-demo.domdev.su
# результат: build/aab/aurora-<version>.aab
```

Если переменные с `export` сохранены локально в `.android-signing/signing.env`,
вместо ручного ввода можно выполнить `source .android-signing/signing.env`.
Для CI передавайте эти переменные через хранилище секретов. Keystore монтируется
в контейнер только для чтения, пароли передаются через окружение, без аргументов
Gradle. Скрипт проверяет подпись AAB и соответствие сертификата выбранному ключу
до копирования артефакта в `build/aab/`. Без параметров подписи release не собирается;
обычный debug APK по-прежнему собирается без них.

Версия берётся из `frontend/package.json`. По умолчанию `versionCode` вычисляется
как `major * 10000 + minor * 100 + patch` (minor и patch должны быть меньше 100).
Для повторной загрузки той же версии увеличьте код вручную:

```bash
AURORA_ANDROID_VERSION_CODE=104 task android-aab
```

Каждая новая загрузка в Google Play требует ещё не использованный `versionCode`;
для обновления установленной версии он должен быть больше предыдущего.
`.android-signing/`, `*.jks` и `*.keystore` исключены из Git. Сохраните резервную
копию ключа, alias и паролей вне репозитория. После сборки загрузите AAB в
Play Console → Testing → Internal testing и настройте Play App Signing.

## Разработка

```bash
task dev          # бэкенд (cargo run) + Vite dev-сервер параллельно
task test         # cargo test, включая матрицу форм источников (нужен ffmpeg)
task lint         # clippy + fmt --check
```

Фронтенд: `npm run build` в `frontend/` прогоняет `svelte-check` и stylelint.

Проверка стриминга на реальной библиотеке:
```bash
cargo test real_library_conformance -- --ignored
```

Документация:
- [`CLAUDE.md`](CLAUDE.md) — архитектура бэкенда и фронтенда
- [`docs/decisions/`](docs/decisions/) — принятые решения по ffmpeg, HLS и дизайн-системе;
  прочитать перед изменением рецептов ffmpeg или сегментации
- [`docs/design-system.md`](docs/design-system.md) — дизайн-система UI
- [`ROADMAP.md`](ROADMAP.md) — план развития
- [`METRICS.md`](METRICS.md) — метрики

## Лицензия

[BSD 3-Clause](LICENSE)
