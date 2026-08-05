# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Aurora is a self-hosted media server (movies + TV series) with a Rust/Axum backend and a Svelte 5 SPA frontend. Users authenticate, browse a library scanned from disk, and stream video in the browser with on-the-fly audio transcoding for browser-incompatible codecs. Docs (`readme.md`, `ROADMAP.md`, `METRICS.md`) are in Russian; code comments are in Russian.

## Commands

Backend (from repo root):
- `cargo run` — start the server (default port 4000; reads `.env`)
- `cargo build --release`
- `cargo test` / `cargo test <name>` — run one test
- `cargo clippy` / `cargo fmt`

Frontend (from `frontend/`):
- `npm run dev` — Vite dev server
- `npm run build` — `svelte-check && vite build` → outputs to `frontend/dist/`, which the backend serves as static files

Full stack: `docker compose up --build` (serves everything on `:4000`).

`ffmpeg` and `ffprobe` must be on `PATH` — `ffprobe` for metadata extraction, `ffmpeg` for transcoding.

## Configuration (env vars)

- `PORT` (default 4000)
- `DATABASE_URL` or `DB_PATH` (default `sqlite:./aurora.db`)
- `MEDIA_PATHS` — comma-separated library directories; movie vs. series is decided by `scanner.rs` from file content patterns, not from the path name, so one root directory is enough
- `STATIC_DIR` — dev override: if the directory exists, static files are read from disk; otherwise the copy of `frontend/dist` embedded into the binary is served
- `JWT_SECRET` — **defaults to a hardcoded dev key; must be set in production**
- `LOG_LEVEL`
- `TMDB_API_KEY` — TMDB v3 key; without it cover art is disabled and the UI falls back to generated placeholders
- `TMDB_LANGUAGE` (default `ru-RU`)
- `CACHE_DIR` (default `./cache`) — downloaded posters/backdrops live in `<CACHE_DIR>/images`

## Architecture

Backend is a flat module set under `src/`, wired in `main.rs` → shared `AppState { config, db, metrics }` (Arc-cloned into Axum handlers):

- `server.rs` — all Axum routes and handlers. API under `/api/*`; everything else falls back to `ServeDir` on the built SPA (SPA routing via `index.html` fallback). This is the file to touch for any endpoint change.
- `database.rs` — SQLx (SQLite) data access; migrations run at startup from `migrations/`.
- `models.rs` — domain types (`MediaItem`, `Library`, `MediaType`, `PlaybackState`, episode/metadata structs).
- `scanner.rs` — walks `MEDIA_PATHS` (walkdir), parses filenames into media items. Classification: a file with an `SxxEyy`-style match becomes an episode, episode groups are merged by common title prefix (`merge_by_common_prefix`), and ≥2 `part N` files sharing a directory/title/year collapse into one series (`split_multipart_movies`); everything else is a movie.
- `ffmpeg.rs` — the only place that names the `ffmpeg`/`ffprobe` binaries and their flags. Each job is a struct with a `to_args()`; flag order is significant and locked by unit tests. Never build ffmpeg argument lists anywhere else.
- `metadata.rs` — parses the `ffprobe` JSON from `ffmpeg::probe_format_json` (duration, resolution, video/audio codec, bitrate).
- `transcoder.rs` — `AudioTranscoder` spawns `ffmpeg` and streams fragmented MP4 (`-c:v copy`, audio → AAC stereo) as an async body stream; `HlsManager` generates HLS segments lazily.
- `segments.rs` — `SegmentMap` is `Keyframes | Grid`; a `Grid` map is only valid when the video is re-encoded. The copy-profile cutter takes the `Keyframes` boundaries by type, so the combination that caused the desync bug does not compile.
- `auth.rs` — JWT (jsonwebtoken, 7-day expiry) + bcrypt password hashing; token extracted from `Authorization` header.
- `tmdb.rs` — TMDB client: title search (`search/movie`, `search/tv`) + downloading posters/backdrops into `<CACHE_DIR>/images`. Enrichment runs in the background after every scan and at startup for items with `tmdb_synced_at IS NULL`; images are served from `/api/media/:id/poster|backdrop` (public, like streams — `<img>` can't send an Authorization header).
- `metrics.rs` — `MetricsCollector` (sysinfo) for process/system stats; logged every 60s and exposed at `/api/metrics`. Also `SEGMENT_DRIFT`: an invariant counter over "a served segment lasts exactly what the map promises" — `mismatched > 0` is a bug even with no user complaints.

- `testkit.rs` (`#[cfg(test)]`) — the axes along which real files differ (`TimestampForm`, `GopForm`, `VideoForm`, `AudioForm`), `Shape::matrix()` of fixtures generated on the fly, and `assert_playable` — the one playability checklist used by every streaming test. **Never write a test against a hand-rolled fixture: add a `Shape` instead.** A fixture healthier than a real release is why three bugs shipped green.

Hard-won decisions live in `docs/decisions/` — read them before touching ffmpeg recipes or HLS segmentation, and add a record rather than rewriting one when a decision changes.

Streaming tests, in order of what they buy you:
- `cargo test` — includes `every_source_shape_produces_playable_segments`, the full matrix (~10 s, needs ffmpeg).
- `cargo test real_library_conformance -- --ignored` — runs the same checklist against one real file per `(video_codec, audio_codec, pix_fmt)` group in `data/aurora.db`. Its job is to find a source shape missing from the matrix; when it does, add the axis to `Shape::matrix()` before fixing anything.

### Streaming model (important)

Two endpoints:
- `/api/media/:id/stream` — direct file streaming with HTTP Range support, capped at a 10 MB chunk per request (`MAX_CHUNK_SIZE`); the `:id` may be either a media item or an episode.
- `/api/media/:id/stream-transcoded` — checks the audio codec; if it's `ac3`/`dts`/`truehd`/`eac3` (not browser-playable) it transcodes via `ffmpeg` in ~120s duration chunks, seeking with `-ss` from a `start` (seconds) query param or a byte-Range converted to seconds. Otherwise it delegates to the plain `stream_handler`. Do not byte-truncate the transcoded stream — it breaks the MP4 container; duration is bounded by ffmpeg `-t`.

Note: `stream*` endpoints are intentionally public (token accepted via query but not verified) so the `<video>` tag can load them; progress endpoints (`/api/media/:id/progress`) do require a valid JWT.

### Data model

`users`, `libraries`, `media_items`, `episodes` (belong to a series `media_item`), `playback_state` (per user+media resume position). Schema in `migrations/001_initial_schema.sql` — schema changes go in a new migration file.

## Frontend

Vite + Svelte 5 (runes) + TypeScript. No SvelteKit, no CSS framework — plain SPA bundle.

- `src/api/client.ts` (axios) talks to the backend; on 401 it calls the handler registered via `onUnauthorized` instead of a hard redirect (a hard redirect would break hash routing in the packaged webOS app).
- `src/lib/router.svelte.ts` — small hand-written router (~90 lines). History mode for the web, hash mode when `apiClient.isPackagedApp()`. Exports `router`, `navigate()` and the `link` action.
- `src/lib/auth.svelte.ts` — `auth` rune store holding the JWT state (replaces React context).
- `src/lib/spatialNavigation.ts`, `src/lib/pointerMode.ts` — TV D-pad and Magic Remote handling; both are `init*()` functions returning a cleanup, called from `$effect` in `App.svelte`.
- Styling: the design system is documented in **`docs/design-system.md`** — read it before adding any style. `src/tokens.css` is the only file where raw colours and sizes are allowed (palette → semantic → per-component layers); `src/index.css` holds the base and the shared classes (`.page`, `.container`, `.app-header`, `.btn` + modifiers, `.input`, `.alert`, `.badge`, `.progress`, `.media-grid`, `.poster`, `.hero`, `.surface-glass`, `.marker-resume`, `.auth-*`). Everything else lives in per-component `<style>` blocks (scoped by Svelte). No utility classes in markup, no `px` sizes (the root `font-size` scales the whole UI, 22px on TV). Reusable markup is a component, not a copied class — check `src/components/` (`AppHeader`, `Modal`, `Section`/`Row`, `ScreenMessage`, `DetailBackdrop`, `MediaTile`) before writing new markup. `npm run lint:css` (stylelint, part of `npm run build`) enforces this; per-line `stylelint-disable-next-line … -- reason` for exceptions, never a config change. Background: `docs/decisions/0004-design-system-layers.md`.
- Routes in `src/routes/` (`Login`, `Library`, `MovieDetail`, `SeriesDetail`, `Watch`, `ServerSetup`), components in `src/components/`; player logic in `components/VideoPlayer.svelte`.

Route params are passed to route components as props (`<MovieDetail id={route.params.id} />`), not read from a hook.

Build output in `frontend/dist/` is what production serves — rebuild after frontend changes.
