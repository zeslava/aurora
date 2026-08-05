<script lang="ts">
  import { apiClient } from '../api/client';
  import { navigate } from '../lib/router.svelte';
  import AppHeader from '../components/AppHeader.svelte';
  import DetailBackdrop from '../components/DetailBackdrop.svelte';
  import ScreenMessage from '../components/ScreenMessage.svelte';
  import TitleEditor from '../components/TitleEditor.svelte';
  import type { MediaItem, PlaybackState } from '../types';

  interface Props {
    id: string;
  }

  let { id }: Props = $props();

  let media = $state<MediaItem | null>(null);
  let progress = $state<PlaybackState | null>(null);
  let loading = $state(true);

  $effect(() => {
    const mediaId = id;
    let active = true;

    const loadData = async () => {
      try {
        loading = true;
        const [mediaData, progressData] = await Promise.all([
          apiClient.getMediaById(mediaId),
          apiClient.getProgress(mediaId).catch(() => null),
        ]);
        if (!active) return;

        media = mediaData;
        progress = progressData;

        // Если это сериал, редиректим на SeriesDetail
        if (mediaData.type === 'tvseries') {
          navigate(`/series/${mediaId}`, { replace: true });
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

  const handleWatch = () => {
    navigate(`/watch/${id}`);
  };

  const formatDuration = (seconds?: number) => {
    if (!seconds) return 'Неизвестно';
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return `${hours}ч ${minutes}м`;
  };

  // Кодек в верхнем регистре — так читабельнее
  const formatCodec = (codec?: string) => (codec ? codec.toUpperCase() : '');

  let progressPercent = $derived(
    progress && media ? Math.round((progress.position / (media.duration || 1)) * 100) : 0
  );
</script>

{#if loading}
  <ScreenMessage text="Загрузка..." />
{:else if !media}
  <ScreenMessage text="Медиа не найдено" />
{:else}
  <div class="page">
    <DetailBackdrop backdrop={media.backdrop} title={media.title} />

    <AppHeader transparent>
      <button onclick={() => navigate('/library')} class="btn--back">← Назад к библиотеке</button>
    </AppHeader>

    <!-- Hero -->
    <div class="hero">
      <div class="container hero__content">
        <TitleEditor {media} onupdate={(updated) => (media = updated)} />
        {#if media.year}
          <p class="hero__meta">{media.year}</p>
        {/if}

        <!-- Кнопки -->
        <div class="hero__actions">
          {#if progress && progressPercent > 5}
            <button onclick={handleWatch} class="btn btn--primary btn--large">Продолжить просмотр</button>
            <button onclick={handleWatch} class="btn btn--large">Начать сначала</button>
          {:else}
            <button onclick={handleWatch} class="btn btn--primary btn--large">Смотреть</button>
          {/if}
        </div>
      </div>
    </div>

    <div class="container detail">
      <div class="detail__info">
        <!-- Прогресс -->
        {#if progress && progressPercent > 5}
          <div class="detail__progress">
            <div class="detail__progress-head">
              <span>Прогресс просмотра</span>
              <span>{progressPercent}%</span>
            </div>
            <div class="progress">
              <div class="progress__bar" style="width: {progressPercent}%"></div>
            </div>
          </div>
        {/if}

        <!-- Детали -->
        <dl class="specs">
          <div class="specs__row">
            <dt>Длительность:</dt>
            <dd>{formatDuration(media.duration)}</dd>
          </div>
          {#if media.resolution}
            <div class="specs__row">
              <dt>Разрешение:</dt>
              <dd>{media.resolution}</dd>
            </div>
          {/if}
          {#if media.video_codec}
            <div class="specs__row">
              <dt>Видео кодек:</dt>
              <dd>{formatCodec(media.video_codec)}</dd>
            </div>
          {/if}
          {#if media.audio_codec}
            <div class="specs__row">
              <dt>Аудио кодек:</dt>
              <dd>{formatCodec(media.audio_codec)}</dd>
            </div>
          {/if}
          {#if media.video_bitrate}
            <div class="specs__row">
              <dt>Битрэйт:</dt>
              <dd>{Math.round(media.video_bitrate / 1000000)} Мбит/с</dd>
            </div>
          {/if}
        </dl>
      </div>
    </div>
  </div>
{/if}

<style>
  .page {
    position: relative;
    isolation: isolate;
  }

  /* Кадр рисует фон страницы — собственная подложка и градиент не нужны */
  .hero {
    background: none;
  }

  .hero::after {
    display: none;
  }

  .detail {
    padding-bottom: var(--space-8);
  }

  .detail__info {
    flex: 1;
  }

  .detail__progress {
    margin-bottom: var(--space-6);
  }

  .detail__progress-head {
    display: flex;
    justify-content: space-between;
    margin-bottom: var(--space-2);
    color: var(--text-muted);
  }

  .specs {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    margin: 0;
  }

  .specs__row {
    display: flex;
    gap: var(--space-2);
  }

  .specs__row dt {
    color: var(--text-muted);
  }

  .specs__row dd {
    margin: 0;
  }

</style>
