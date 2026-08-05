# Multi-stage build для Aurora медиасервера

# Этап 1: Сборка фронтенда
FROM node:20-alpine AS frontend-builder

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

# Этап 3: Финальный образ
FROM alpine:latest

# Устанавливаем необходимые зависимости
RUN apk add --no-cache \
    sqlite \
    ffmpeg \
    ca-certificates \
    tzdata \
    wget

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
