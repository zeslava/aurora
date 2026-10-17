# Multi-stage build для Aurora медиасервера

# Этап 1: Сборка фронтенда
# dist не зависит от архитектуры — собираем на платформе раннера, без эмуляции
FROM --platform=$BUILDPLATFORM node:20-alpine AS frontend-builder

WORKDIR /app/frontend

# Копируем package файлы
COPY frontend/package*.json ./

# Устанавливаем зависимости
RUN npm ci

# Копируем исходники фронтенда
COPY frontend/ ./

# Собираем фронтенд
RUN npm run build

# Этап 2: Сборка Rust приложения
FROM rust:1.92-alpine AS rust-builder

# Устанавливаем зависимости для сборки
RUN apk add --no-cache musl-dev sqlite-dev

WORKDIR /app

ENV CARGO_PROFILE_RELEASE_STRIP=true

# Копируем Cargo файлы
COPY Cargo.toml Cargo.lock ./

# Создаем фиктивный src/main.rs для кэширования зависимостей
RUN mkdir -p src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Копируем исходники. Миграции и собранный фронтенд вшиваются в бинарь на этапе
# компиляции (sqlx::migrate! и rust-embed), поэтому нужны здесь, а не в финальном образе.
COPY src/ ./src/
COPY migrations/ ./migrations/
COPY --from=frontend-builder /app/frontend/dist ./frontend/dist

# Сбрасываем кэш заглушки, иначе cargo не пересоберёт бинарь
RUN rm -f target/release/aurora target/release/deps/aurora-* && \
    cargo build --release

# Этап 3: Минимальный ffmpeg. Пакет из apk тянет ~120 МБ зависимостей (x265, aom, svt-av1,
# X11) ради энкодеров, которые нам не нужны. Декодеры, демуксеры и парсеры — все встроенные
# (внешних библиотек у них нет): открыть надо любой источник. Энкодеры и муксеры — только те,
# что называет ffmpeg.rs. Версия — та же 6.1, на которой проходят тесты.
FROM alpine:3.22 AS ffmpeg-builder

ARG FFMPEG_VERSION=6.1.2

RUN apk add --no-cache build-base nasm pkgconf x264-dev zlib-dev xz

WORKDIR /src

RUN wget -qO- https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz | tar xJ --strip-components=1

RUN ./configure \
        --prefix=/opt/ffmpeg \
        --enable-gpl --enable-libx264 --enable-zlib \
        --enable-shared --disable-static \
        --disable-debug --disable-doc --disable-ffplay \
        --disable-autodetect --disable-network \
        --disable-everything \
        --enable-protocol=file,pipe \
        --enable-demuxers --enable-decoders --enable-parsers --enable-bsfs \
        --enable-encoder=aac,libx264 \
        --enable-muxer=mp4,hls,mpegts,null \
        --enable-filter=buffer,buffersink,abuffer,abuffersink,format,aformat,null,anull,scale,aresample,yadif,setpts,asetpts,trim,atrim && \
    make -j"$(nproc)" && \
    make install && \
    cd /opt/ffmpeg && \
    rm -rf include share lib/pkgconfig lib/*.so && \
    strip bin/* lib/*

# Этап 4: Финальный образ
FROM alpine:3.22

RUN apk add --no-cache x264-libs

# Симлинки lib*.so.N внутри каталога COPY сохраняет — библиотеки не дублируются
COPY --from=ffmpeg-builder /opt/ffmpeg /usr/local

# Создаем пользователя для запуска приложения
RUN addgroup -g 1000 aurora && \
    adduser -D -u 1000 -G aurora aurora

WORKDIR /app

# Копируем собранный бинарник из этапа сборки
COPY --from=rust-builder /app/target/release/aurora /app/aurora

# Создаем директории для данных
RUN mkdir -p /app/data /app/media && \
    chown -R aurora:aurora /app

# Переключаемся на пользователя aurora
USER aurora

# Переменные окружения по умолчанию
ENV PORT=4000
ENV DATABASE_URL=sqlite:/app/data/aurora.db
ENV LOG_LEVEL=info

# Открываем порт
EXPOSE 4000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:4000/health || exit 1

# Запускаем приложение
CMD ["./aurora"]
