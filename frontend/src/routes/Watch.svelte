<script lang="ts">
  import { apiClient } from '../api/client';
  import { navigate, router } from '../lib/router.svelte';
  import VideoPlayer from '../components/VideoPlayer.svelte';
  import ScreenMessage from '../components/ScreenMessage.svelte';
  import { isAtTail, isWatchedToEnd } from '../lib/playback';
  import type { MediaItem, PlaybackState, SeriesEpisodesResponse, Episode } from '../types';

  interface Props {
    id: string;
    season?: string;
    episode?: string;
  }

  let { id, season, episode }: Props = $props();

  let media = $state<MediaItem | null>(null);
  let progress = $state<PlaybackState | null>(null);
  let episodes = $state<SeriesEpisodesResponse | null>(null);
  let currentEpisode = $state<Episode | null>(null);
  let loading = $state(true);
  let streamUrl = $state('');

  // Экран, с которого открыли плеер (главная или страница сериала)
  const backTo = router.previous ?? '/library';

  $effect(() => {
    const mediaId = id;
    const seasonParam = season;
    const episodeParam = episode;
    let active = true;

    const loadData = async () => {
      try {
        loading = true;
        media = null;
        progress = null;
        episodes = null;
        currentEpisode = null;
        streamUrl = '';

        const mediaData = await apiClient.getMediaById(mediaId);
        if (!active || !mediaData) return;

        media = mediaData;
        // Офсет в URL проставляет сам плеер (стартовая позиция и перемотка),
        // здесь URL задаёт только источник и тип потока.
        streamUrl = apiClient.getHlsUrl(mediaId);

        // Если это сериал, загружаем эпизоды
        if (mediaData.type === 'tvseries') {
          const episodesData = await apiClient.getSeriesEpisodes(mediaId);
          if (!active) return;
          episodes = episodesData;

          // Определяем текущий эпизод
          if (seasonParam && episodeParam && episodesData) {
            const seasonNum = parseInt(seasonParam);
            const episodeNum = parseInt(episodeParam);
            const episodeList = episodesData.seasons[seasonNum.toString()] || [];
            const foundEpisode = episodeList.find(
              (e) => e.season_number === seasonNum && e.episode_number === episodeNum
            );
            if (foundEpisode) {
              currentEpisode = foundEpisode;
              // Для эпизодов используем ID эпизода для прогресса и стрима
              const episodeProgress = await apiClient.getProgress(foundEpisode.id).catch(() => null);
              if (!active) return;
              if (episodeProgress) progress = episodeProgress;
              streamUrl = apiClient.getHlsUrl(foundEpisode.id);
            }
          }
        } else {
          // Фильм: прогресс хранится по id самого медиа
          const progressData = await apiClient.getProgress(mediaId).catch(() => null);
          if (!active) return;
          progress = progressData;
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

  // Путь к следующему эпизоду в иерархии сериала, либо null (фильм / последний эпизод)
  let nextEpisodePath = $derived.by((): string | null => {
    if (media?.type !== 'tvseries' || !currentEpisode || !episodes) return null;

    const currentSeason = currentEpisode.season_number;
    const currentEpisodeNum = currentEpisode.episode_number;
    const seasonEpisodes = episodes.seasons[currentSeason.toString()] || [];
    const nextEpisode = seasonEpisodes.find((e) => e.episode_number === currentEpisodeNum + 1);

    if (nextEpisode) {
      return `/watch/${id}/${currentSeason}/${nextEpisode.episode_number}`;
    }

    // Следующий сезон
    const seasons = Object.keys(episodes.seasons)
      .map(Number)
      .sort((a, b) => a - b);
    const nextSeasonIndex = seasons.indexOf(currentSeason) + 1;
    if (nextSeasonIndex < seasons.length) {
      const nextSeason = seasons[nextSeasonIndex];
      const nextSeasonEpisodes = episodes.seasons[nextSeason.toString()] || [];
      if (nextSeasonEpisodes.length > 0) {
        return `/watch/${id}/${nextSeason}/${nextSeasonEpisodes[0].episode_number}`;
      }
    }
    return null;
  });

  const goToNextEpisode = () => {
    if (nextEpisodePath) navigate(nextEpisodePath);
  };

  // Воспроизводимая сущность: эпизод, если открыта серия, иначе сам фильм
  let playable = $derived(currentEpisode || media);

  // Отметка «просмотрено» для бейджа/кнопки «заново»: явный completed или хвост файла
  let watchedToEnd = $derived(isWatchedToEnd(progress, playable?.duration));

  // С нуля открываем только то, что доиграно до конца: completed выставляется уже на 90%,
  // и по нему стартовать заново — значит терять сохранённую позицию при повторном заходе
  let atTail = $derived(isAtTail(progress, playable?.duration));

  let startTime = $derived(atTail ? 0 : progress?.position || 0);
</script>

{#if loading}
  <ScreenMessage text="Загрузка..." cinema />
{:else if !media || !streamUrl || !playable}
  <ScreenMessage text="Медиа не найдено" cinema />
{:else}
  <div class="watch">
    <!-- Видеоплеер -->
    <div class="watch__player">
      {#key streamUrl}
        <VideoPlayer
          src={streamUrl}
          mediaId={playable.id}
          duration={playable.duration}
          onEnded={goToNextEpisode}
          onNextEpisode={nextEpisodePath ? goToNextEpisode : undefined}
          autoplay={!atTail}
          watched={watchedToEnd}
          {startTime}
          audioCodec={playable.audio_codec}
        >
          {#snippet overlay()}
            <button onclick={() => navigate(backTo)} class="watch__back">← Назад</button>
            <div class="watch__meta">
              <h2 class="watch__title">{media?.title}</h2>
              {#if currentEpisode}
                <p class="watch__episode">
                  Сезон {currentEpisode.season_number}, Эпизод {currentEpisode.episode_number}
                </p>
              {/if}
            </div>
          {/snippet}
        </VideoPlayer>
      {/key}
      <!-- Диагностическая информация (только если кодек не определен) -->
      {#if !playable.audio_codec}
        <div class="watch__warning">⚠️ Аудио кодек не определен - возможно, видео без звука</div>
      {/if}
    </div>

  </div>
{/if}

<style>
  .watch {
    min-height: 100vh;
    background-color: var(--bg-cinema);
  }

  .watch__player {
    width: 100%;
    height: 100vh;
  }

  .watch__warning {
    position: absolute;
    left: var(--space-4);
    bottom: var(--space-20);
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    background-color: var(--scrim);
    color: var(--warning);
    font-size: var(--text-sm);
  }

  .watch__back {
    display: block;
    margin-bottom: var(--space-2);
    padding: var(--space-2) var(--space-4);
    border: none;
    border-radius: var(--radius);
    background-color: var(--scrim);
    color: var(--text);
    cursor: pointer;
    transition: background-color var(--transition);
  }

  .watch__back:hover {
    background-color: var(--scrim-strong);
  }

  .watch__meta {
    padding: var(--space-2) var(--space-4);
    border-radius: var(--radius);
    background-color: var(--scrim);
  }

  .watch__title {
    font-size: var(--text-xl);
    font-weight: var(--weight-bold);
  }

  .watch__episode {
    color: var(--text-on-media);
    font-size: var(--text-sm);
  }
</style>
