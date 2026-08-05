# Graph Report - .  (2026-08-04)

## Corpus Check
- 85 files · ~54,494 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 846 nodes · 1909 edges · 48 communities (36 shown, 12 thin omitted)
- Extraction: 98% EXTRACTED · 2% INFERRED · 0% AMBIGUOUS · INFERRED: 33 edges (avg confidence: 0.82)
- Token cost: 166,440 input · 0 output

## Community Hubs (Navigation)
- Server HTTP Handlers & Routes
- Database Layer (SQLx)
- Audio Transcoder & HLS Manager
- Stylelint Config
- Segment Map / HLS Boundaries
- TMDB Metadata Client
- Media Filename Scanner
- Frontend Package Dependencies
- FFmpeg Job Args
- Frontend TS Config
- Metrics Collector
- Streaming Test Fixtures (testkit)
- Domain Models
- Auth & Config
- ApiClient Auth/Pairing Methods
- Main Entry & TV Pairing Manager
- ApiClient Media Fetch Methods
- Frontend Auth/Login Flow
- Detail Routes & Video Player
- Frontend Router
- TV Spatial Navigation
- Library UI Components
- HLS VOD Playback Docs
- Design System Docs & Tokens
- HLS Segmentation ADRs & Post-mortem
- Segments/Transcoder Migration Plan
- Embedded Static Files
- Poster Art Generation
- Frontend Auth Store
- Aurora Branding & webOS Icon
- AppState & Streaming Model Docs
- Docker/webOS Deployment Docs
- TV Pointer Mode
- Metrics Docs
- HLS Cut Probe Script
- Aurora Project & Large Icon
- Auth Module & Security Roadmap
- webOS Build Script
- Seek Check Script
- database.rs Doc Note
- models.rs Doc Note
- scanner.rs Doc Note
- tmdb.rs Doc Note

## God Nodes (most connected - your core abstractions)
1. `AppState` - 54 edges
2. `Database` - 46 edges
3. `ApiClient` - 33 edges
4. `get()` - 26 edges
5. `StreamProfile` - 21 edges
6. `rules` - 18 edges
7. `HlsManager` - 18 edges
8. `compilerOptions` - 17 edges
9. `SegmentMap` - 17 edges
10. `MetricsCollector` - 16 edges

## Surprising Connections (you probably didn't know these)
- `Aurora MVP Plan` --semantically_similar_to--> `1.2 HLS Migration (Segment Map VOD)`  [INFERRED] [semantically similar]
  README.md → ROADMAP.md
- `docker-compose aurora service` --implements--> `Aurora Media Server`  [INFERRED]
  docker-compose.yml → CLAUDE.md
- `HLS VOD Segment Map Architecture` --rationale_for--> `Streaming Model (stream / stream-transcoded)`  [INFERRED]
  PLAYBACK.md → CLAUDE.md
- `HLS Copy-Profile Segment Desync Bug (Post-mortem)` --semantically_similar_to--> `Source Shape Matrix Fixture Strategy`  [INFERRED] [semantically similar]
  РАЗБОР_ПОЛЕТОВ.md → docs/decisions/0003-source-shape-matrix.md
- `frontend/index.html SPA entry` --shares_data_with--> `Aurora Media Server`  [INFERRED]
  frontend/index.html → CLAUDE.md

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **HLS Copy-Segmentation Debugging & Decision Chain** — razbor_poletov_hls_segment_desync_bug, docs_decisions_0001_hls_copy_segmentation_hls_copy_segmentation, docs_decisions_0002_ffmpeg_port_ffmpeg_port, docs_decisions_0003_source_shape_matrix_source_shape_matrix [EXTRACTED 1.00]
- **HLS VOD Migration Documentation Set** — playback_md_hls_vod_architecture, playback_plan_md_review_blockers, roadmap_hls_migration [EXTRACTED 1.00]
- **Design System Token Layer Enforcement** — docs_design_system_design_system, docs_design_system_tokens_css, docs_design_system_index_css, docs_decisions_0004_design_system_layers_design_system_layers [INFERRED 0.85]

## Communities (48 total, 12 thin omitted)

### Community 0 - "Server HTTP Handlers & Routes"
Cohesion: 0.06
Nodes (100): Body, HeaderMap, Json, Query, ScanJobs, AppState, AuthResponse, backdrop_handler() (+92 more)

### Community 1 - "Database Layer (SQLx)"
Cohesion: 0.07
Nodes (36): Episode, HashSet, MediaFileInfo, SqlitePool, SqliteRow, Database, image_url(), insert_file() (+28 more)

### Community 2 - "Audio Transcoder & HLS Manager"
Cohesion: 0.10
Nodes (37): AsyncMutex, Child, ChildStdout, Context, Drop, Duration, Item, JoinHandle (+29 more)

### Community 3 - "Stylelint Config"
Cohesion: 0.05
Nodes (47): background-color, color, font-size, font-weight, /^(padding|margin|gap|border-radius)/, transition-duration, z-index, extends (+39 more)

### Community 4 - "Segment Map / HLS Boundaries"
Cohesion: 0.11
Nodes (24): Boundaries, build_segment_map(), collapse_keyframes(), copy_playlist_uses_ts_without_init_map(), grid_map(), grid_map_covers_whole_duration(), keyframes_collapse_into_segments_of_at_least_four_seconds(), map_source_must_match_profile() (+16 more)

### Community 5 - "TMDB Metadata Client"
Cohesion: 0.11
Nodes (28): Client, Into, profile_keys_are_distinct(), candidates(), cross_type_candidate_requires_title_match(), falls_back_to_popularity(), is_cyrillic(), matches_title() (+20 more)

### Community 6 - "Media Filename Scanner"
Cohesion: 0.14
Nodes (33): BTreeMap, Regex, Classified, classify(), clean_title(), episode_pattern(), extract_series_title(), is_video_file() (+25 more)

### Community 7 - "Frontend Package Dependencies"
Cohesion: 0.06
Nodes (35): axios, dependencies, axios, hls.js, devDependencies, postcss-html, stylelint, stylelint-config-standard (+27 more)

### Community 8 - "FFmpeg Job Args"
Cohesion: 0.15
Nodes (29): AsRef, OsStr, OsString, audio_args(), audio_recipe_is_identical_across_jobs(), AudioTranscodeStream, base_args(), command() (+21 more)

### Community 9 - "Frontend TS Config"
Cohesion: 0.07
Nodes (27): compilerOptions, allowImportingTsExtensions, isolatedModules, lib, module, moduleResolution, noEmit, noFallthroughCasesInSwitch (+19 more)

### Community 10 - "Metrics Collector"
Cohesion: 0.14
Nodes (16): AtomicU64, Mutex, Pid, drift_counts_only_deviations_beyond_tolerance(), Metrics, MetricsCollector, ProcessMetrics, Arc (+8 more)

### Community 11 - "Streaming Test Fixtures (testkit)"
Cohesion: 0.16
Nodes (16): assert_playable(), AudioForm, GopForm, Packet, probe_packets(), probe_video_params(), Option, Path (+8 more)

### Community 12 - "Domain Models"
Cohesion: 0.22
Nodes (21): extract_metadata(), parse_ffprobe_output(), Path, Result, Value, ContinueWatchingItem, Episode, Library (+13 more)

### Community 13 - "Auth & Config"
Cohesion: 0.15
Nodes (20): BcryptError, Error, HeaderValue, Claims, create_token(), extract_token(), get_secret_key(), hash_password() (+12 more)

### Community 15 - "Main Entry & TV Pairing Manager"
Cohesion: 0.16
Nodes (14): RwLock, main(), Result, gen_code(), PairingEntry, PairingManager, PollResult, Arc (+6 more)

### Community 16 - "ApiClient Media Fetch Methods"
Cohesion: 0.12
Nodes (11): ContinueWatchingItem, Episode, HomeResponse, Library, MediaFileInfo, MediaItem, MediaType, PlaybackState (+3 more)

### Community 17 - "Frontend Auth/Login Flow"
Cohesion: 0.22
Nodes (3): normalizeUrl(), LoginRequest, RegisterRequest

### Community 18 - "Detail Routes & Video Player"
Cohesion: 0.19
Nodes (4): isAtTail(), isWatchedToEnd(), progressPercent, percentOf()

### Community 19 - "Frontend Router"
Cohesion: 0.20
Nodes (10): currentPath(), isWatch(), link(), match(), navigate(), PATTERNS, RouteName, Router (+2 more)

### Community 20 - "TV Spatial Navigation"
Cohesion: 0.30
Nodes (11): BACK_KEYS, center(), collectFocusable(), collectRows(), Direction, FOCUSABLE_SELECTOR, initSpatialNavigation(), isTextInput() (+3 more)

### Community 22 - "HLS VOD Playback Docs"
Cohesion: 0.22
Nodes (10): GET /api/media/:id/hls/playlist.m3u8, HLS VOD Segment Map Architecture, Multi-device Playback Support, Old Offset-Session HLS Model, Stream Profile (copy/a/v/av), Step 7 Implementation Review Blockers, VideoPlayer.svelte refactor, 1.1 Data Model (media_files/episodes) (+2 more)

### Community 23 - "Design System Docs & Tokens"
Cohesion: 0.25
Nodes (9): Aurora Media Server, Design System Token Layers + Input Modes, Aurora Design System, --focus-ring Usage Rule, index.css, tokens.css, frontend/index.html SPA entry, Taskfile build task (+1 more)

### Community 24 - "HLS Segmentation ADRs & Post-mortem"
Cohesion: 0.33
Nodes (9): ffmpeg.rs, metadata.rs, testkit.rs, HLS Copy Segmentation (per-segment ffmpeg runs), ffmpeg.rs Port + Segment Drift Metric, Source Shape Matrix Fixture Strategy, Decisions Log Index, scripts/hls-cut-probe.sh (+1 more)

### Community 25 - "Segments/Transcoder Migration Plan"
Cohesion: 0.29
Nodes (7): SegmentMap, segments.rs, transcoder.rs, HlsManager (run-based), migrations/004_segment_index.sql, segments.rs (step 0), run_copy_segments

### Community 26 - "Embedded Static Files"
Cohesion: 0.38
Nodes (6): EmbeddedFile, Assets, file_response(), Response, serve(), Uri

### Community 29 - "Aurora Branding & webOS Icon"
Cohesion: 0.47
Nodes (6): Aurora Design System (docs/design-system.md), Aurora (self-hosted media server), frontend/webos directory (LG webOS packaging), Aurora webOS App Icon, Dark Navy Rounded Square Background, Cyan Letter 'A' Glyph

### Community 30 - "AppState & Streaming Model Docs"
Cohesion: 0.33
Nodes (6): AppState, server.rs, Streaming Model (stream / stream-transcoded), hls_playlist_handler, hls_segment_handler, 5. Качество кода

### Community 31 - "Docker/webOS Deployment Docs"
Cohesion: 0.33
Nodes (6): docker-compose aurora service, Docker Deployment, Aurora MVP Plan, webOS Application Packaging, 2. ТВ-интерфейс (LG webOS), Taskfile webos-deploy task

### Community 32 - "TV Pointer Mode"
Cohesion: 0.60
Nodes (4): D_PAD_KEYS, FOCUSABLE_SELECTOR, initPointerMode(), setMode()

### Community 33 - "Metrics Docs"
Cohesion: 0.50
Nodes (4): metrics.rs, GET /api/metrics, Process Metrics, System Metrics

### Community 34 - "HLS Cut Probe Script"
Cohesion: 0.67
Nodes (3): LC_ALL, probe_recipe(), hls-cut-probe.sh script

## Ambiguous Edges - Review These
- `Aurora webOS App Icon` → `Aurora Design System (docs/design-system.md)`  [AMBIGUOUS]
  frontend/webos/icon.png · relation: likely_follows_palette_of

## Knowledge Gaps
- **122 isolated node(s):** `extends`, `stylelint-config-standard`, `overrides`, `custom-property-pattern`, `selector-class-pattern` (+117 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **12 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **What is the exact relationship between `Aurora webOS App Icon` and `Aurora Design System (docs/design-system.md)`?**
  _Edge tagged AMBIGUOUS (relation: likely_follows_palette_of) - confidence is low._
- **Why does `Database` connect `Database Layer (SQLx)` to `Server HTTP Handlers & Routes`, `Main Entry & TV Pairing Manager`?**
  _High betweenness centrality (0.086) - this node is a cross-community bridge._
- **Why does `AppState` connect `Server HTTP Handlers & Routes` to `Database Layer (SQLx)`, `Audio Transcoder & HLS Manager`, `TMDB Metadata Client`, `Metrics Collector`, `Auth & Config`, `Main Entry & TV Pairing Manager`?**
  _High betweenness centrality (0.075) - this node is a cross-community bridge._
- **Why does `TmdbClient` connect `TMDB Metadata Client` to `Server HTTP Handlers & Routes`?**
  _High betweenness centrality (0.037) - this node is a cross-community bridge._
- **What connects `extends`, `stylelint-config-standard`, `overrides` to the rest of the system?**
  _122 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Server HTTP Handlers & Routes` be split into smaller, more focused modules?**
  _Cohesion score 0.06446280991735537 - nodes in this community are weakly interconnected._
- **Should `Database Layer (SQLx)` be split into smaller, more focused modules?**
  _Cohesion score 0.07403508771929825 - nodes in this community are weakly interconnected._