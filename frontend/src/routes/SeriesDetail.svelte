<script lang="ts">
  import { tick } from 'svelte';
  import { apiClient } from '../api/client';
  import { navigate } from '../lib/router.svelte';
  import SeasonSelector from '../components/SeasonSelector.svelte';
  import AppHeader from '../components/AppHeader.svelte';
  import DetailBackdrop from '../components/DetailBackdrop.svelte';
  import ScreenMessage from '../components/ScreenMessage.svelte';
  import TitleEditor from '../components/TitleEditor.svelte';
  import { isWatchedToEnd } from '../lib/playback';
  import type { MediaItem, SeriesEpisodesResponse, Episode, PlaybackState } from '../types';

  interface Props {
    id: string;
  }

  let { id }: Props = $props();

  let media = $state<MediaItem | null>(null);
  let episodes = $state<SeriesEpisodesResponse | null>(null);
  let selectedSeason = $state(1);
  let loading = $state(true);
  let progressMap = $state<Record<string, PlaybackState>>({});
  // Автопереход к недосмотренной серии делаем один раз на сериал, чтобы не перебивать
  // ручной выбор сезона после подгрузки прогресса
  let resumeApplied = $state(false);
  let resumeEpisodeId = $state<string | null>(null);

  $effect(() => {
    const mediaId = id;
    let active = true;
    resumeApplied = false;

    const loadData = async () => {
      try {
        loading = true;
        const [mediaData, episodesData] = await Promise.all([
          apiClient.getMediaById(mediaId),
          apiClient.getSeriesEpisodes(mediaId),
        ]);
        if (!active) return;

        media = mediaData;
        episodes = episodesData;

        // Выбираем первый доступный сезон
        const seasonNumbers = Object.keys(episodesData.seasons)
          .map(Number)
          .sort((a, b) => a - b);
        if (seasonNumbers.length > 0) {
          selectedSeason = seasonNumbers[0];
        }
      } catch (error) {
        console.error('Ошибка загрузки данных:', error);
      } finally {
        if (active) loading = false;
      }
    };

    loadData();

    return () => {
      active = false;
    };
  });

  // Загружаем прогресс для эпизодов
  $effect(() => {
    const data = episodes;
    if (!data) return;

    let active = true;

    const loadProgress = async () => {
      const allEpisodes = Object.values(data.seasons).flat();
      const results = await Promise.all(
        allEpisodes.map(async (episode) => {
          const progress = await apiClient.getProgress(episode.id).catch(() => null);
          return progress ? { id: episode.id, progress } : null;
        })
      );
      if (!active) return;

      const map: Record<string, PlaybackState> = {};
      results.forEach((result) => {
        if (result) map[result.id] = result.progress;
      });
      progressMap = map;

      if (!resumeApplied) {
        resumeApplied = true;
        await focusResumeEpisode(data, map);
      }
    };

    loadProgress();

    return () => {
      active = false;
    };
  });

  // Первая серия по порядку сезонов/номеров, которая ещё не досмотрена до конца.
  // Если досмотрено всё — возвращаем последнюю, с неё удобнее начать заново
  const findResumeEpisode = (
    data: SeriesEpisodesResponse,
    map: Record<string, PlaybackState>
  ): Episode | null => {
    const ordered = Object.keys(data.seasons)
      .map(Number)
      .sort((a, b) => a - b)
      .flatMap((season) =>
        [...(data.seasons[season.toString()] || [])].sort(
          (a, b) => a.episode_number - b.episode_number
        )
      );

    return (
      ordered.find((episode) => !isWatchedToEnd(map[episode.id], episode.duration)) ??
      ordered[ordered.length - 1] ??
      null
    );
  };

  const focusResumeEpisode = async (
    data: SeriesEpisodesResponse,
    map: Record<string, PlaybackState>
  ) => {
    const target = findResumeEpisode(data, map);
    if (!target) return;

    selectedSeason = target.season_number;
    resumeEpisodeId = target.id;
    await tick();

    const row = document.querySelector<HTMLElement>(`[data-episode-id="${target.id}"] button`);
    if (!row) return;
    row.focus({ preventScroll: true });
    row.scrollIntoView({ block: 'center' });
  };

  const handleWatchEpisode = (episode: Episode) => {
    navigate(`/watch/${id}/${episode.season_number}/${episode.episode_number}`);
  };

  let seasons = $derived(
    episodes
      ? Object.keys(episodes.seasons)
          .map(Number)
          .sort((a, b) => a - b)
      : []
  );
  let currentEpisodes = $derived(episodes?.seasons[selectedSeason.toString()] || []);
  // Постер сезона есть не всегда — фоном тогда останется обложка сериала
  let seasonPoster = $derived(episodes?.season_posters[selectedSeason.toString()] ?? null);

  // Досмотренный эпизод открывается с нуля, но полоса остаётся полной — так строка
  // читается однозначно: серия пройдена целиком
  const percentOf = (episode: Episode, progress?: PlaybackState) => {
    if (isWatchedToEnd(progress, episode.duration)) return 100;
    const episodeDuration = episode.duration || 0;
    return progress && episodeDuration > 0
      ? Math.min(100, Math.round((progress.position / episodeDuration) * 100))
      : 0;
  };
</script>

{#if loading}
  <ScreenMessage text="Загрузка..." />
{:else if !media || !episodes}
  <ScreenMessage text="Сериал не найден" />
{:else}
  <div class="page">
    <DetailBackdrop
      backdrop={seasonPoster}
      fallback={media.backdrop}
      title={media.title}
      icon="📺"
      objectPosition="center top"
    />

    <AppHeader transparent>
      <button onclick={() => navigate('/library')} class="btn--back">← Назад к библиотеке</button>
    </AppHeader>

    <div class="container series">
      <div class="series__head">
        <TitleEditor {media} onupdate={(updated) => (media = updated)} />
        {#if media.year}
          <p class="hero__meta">{media.year}</p>
        {/if}
      </div>

      <SeasonSelector
        {seasons}
        {selectedSeason}
        posters={episodes.season_posters}
        onSelectSeason={(s) => (selectedSeason = s)}
      />

      <!-- Список эпизодов -->
      <div class="series__panel surface-glass">
        <h2 class="series__season-title">Сезон {selectedSeason}</h2>
        <div class="episodes">
          {#each currentEpisodes as episode (episode.id)}
            {@const progress = progressMap[episode.id]}
            {@const progressPercent = percentOf(episode, progress)}
            {@const watched = isWatchedToEnd(progress, episode.duration)}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div
              class="episode"
              class:marker-resume={episode.id === resumeEpisodeId}
              data-episode-id={episode.id}
              onclick={() => handleWatchEpisode(episode)}
            >
              <div class="episode__main">
                <div class="episode__meta">
                  <span class="episode__number">Эпизод {episode.episode_number}</span>
                  {#if watched}
                    <span class="badge badge--success">Просмотрено</span>
                  {/if}
                </div>
                <h3 class="episode__title">
                  {episode.title || `Эпизод ${episode.episode_number}`}
                </h3>
                <!-- Место под полосу занято всегда: её появление не должно двигать строку -->
                <div class="episode__progress">
                  {#if progress && progressPercent > 0}
                    <div class="progress progress--thin">
                      <div class="progress__bar" style="width: {progressPercent}%"></div>
                    </div>
                  {/if}
                </div>
              </div>
              <button class="btn btn--primary">
                {watched ? 'Заново' : progress && progressPercent > 5 ? 'Продолжить' : 'Смотреть'}
              </button>
            </div>
          {/each}
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .page {
    position: relative;
    isolation: isolate;
  }

  .series {
    padding-top: var(--space-4);
    padding-bottom: var(--space-8);
  }

  .series__head {
    padding-block: var(--space-8) var(--space-6);
  }

  /* Подложка — общий .surface-glass; здесь только раскладка */
  .series__panel {
    margin-top: var(--space-6);
    padding: var(--space-6);
  }

  .series__season-title {
    margin-bottom: var(--space-4);
    font-size: var(--text-2xl);
    font-weight: var(--weight-bold);
  }

  .episodes {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .episode {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-4);
    border-radius: var(--radius);
    background-color: var(--surface);
    cursor: pointer;
    transition: background-color var(--transition);
  }

  .episode:hover {
    background-color: var(--surface-2);
  }

  /* Фокус стоит на кнопке внутри строки — подсвечиваем строку целиком,
     иначе обводка вокруг кнопки не читается с дивана */
  .episode:has(:focus-visible) {
    background-color: var(--surface-2);
  }

  @supports not selector(:focus-visible) {
    .episode:focus-within {
      background-color: var(--surface-2);
    }
  }

  .episode__main {
    flex: 1;
  }

  /* min-height держит строку одинаковой с плашкой «Просмотрено» и без неё —
     иначе просмотренные эпизоды выше остальных */
  .episode__meta {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-height: 1.5rem;
    margin-bottom: var(--space-2);
  }

  .episode__number {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }

  .episode__title {
    font-weight: var(--weight-semibold);
  }

  .episode__progress {
    height: 4px;
    margin-top: var(--space-2);
  }
</style>
