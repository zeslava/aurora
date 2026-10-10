<script lang="ts">
  import { auth } from '../lib/auth.svelte';
  import { apiClient } from '../api/client';
  import MediaCard from '../components/MediaCard.svelte';
  import ContinueCard from '../components/ContinueCard.svelte';
  import PairTvModal from '../components/PairTvModal.svelte';
  import AppHeader from '../components/AppHeader.svelte';
  import Section from '../components/Section.svelte';
  import Row from '../components/Row.svelte';
  import ScreenMessage from '../components/ScreenMessage.svelte';
  import type { MediaItem, PlaybackState, HomeResponse } from '../types';

  type FilterType = 'all' | 'movie' | 'tvseries';
  type SortType = 'newest' | 'title' | 'title-desc';

  let media = $state<MediaItem[]>([]);
  let home = $state<HomeResponse | null>(null);
  let progressMap = $state<Record<string, PlaybackState>>({});
  let loading = $state(true);
  let filter = $state<FilterType>('all');
  let sort = $state<SortType>('newest');
  let pairOpen = $state(false);

  const loadMedia = async () => {
    try {
      loading = true;
      media = await apiClient.getMedia();
    } catch (error) {
      console.error('Ошибка загрузки медиа:', error);
    } finally {
      loading = false;
    }
  };

  const loadHome = async () => {
    try {
      home = await apiClient.getHome();
    } catch (error) {
      console.error('Ошибка загрузки главного экрана:', error);
    }
  };

  loadMedia();
  loadHome();

  // Загружаем прогресс для всех медиа с ограничением количества одновременных запросов
  $effect(() => {
    const items = media;
    if (items.length === 0) return;

    let active = true;

    const loadProgress = async () => {
      // Ограничиваем количество одновременных запросов до 10
      const BATCH_SIZE = 10;
      const map: Record<string, PlaybackState> = {};

      for (let i = 0; i < items.length; i += BATCH_SIZE) {
        const batch = items.slice(i, i + BATCH_SIZE);
        const results = await Promise.all(
          batch.map(async (item) => {
            try {
              const progress = await apiClient.getProgress(item.id);
              return progress ? { id: item.id, progress } : null;
            } catch {
              return null;
            }
          })
        );
        if (!active) return;

        results.forEach((result) => {
          if (result) map[result.id] = result.progress;
        });

        // Обновляем прогресс постепенно
        progressMap = { ...map };

        // Небольшая задержка между батчами для снижения нагрузки
        if (i + BATCH_SIZE < items.length) {
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
    };

    loadProgress();

    return () => {
      active = false;
    };
  });

  const handleScan = async () => {
    try {
      await apiClient.scanLibraries();
      await loadMedia();
      await loadHome();
    } catch (error) {
      console.error('Ошибка сканирования:', error);
    }
  };

  // Фильтрация и сортировка
  let filteredAndSortedMedia = $derived.by(() => {
    let filtered = media;

    // Фильтр по типу
    if (filter !== 'all') {
      filtered = filtered.filter((item) => item.type === filter);
    }

    const byTitle = (a: MediaItem, b: MediaItem) => a.title.localeCompare(b.title);
    // Без created_at элемент уходит в конец списка
    const addedAt = (item: MediaItem) => (item.created_at ? Date.parse(item.created_at) : 0);

    switch (sort) {
      case 'newest':
        return [...filtered].sort((a, b) => addedAt(b) - addedAt(a) || byTitle(a, b));
      case 'title':
        return [...filtered].sort(byTitle);
      case 'title-desc':
        return [...filtered].sort((a, b) => byTitle(b, a));
    }
  });

  // Главный экран показываем только без фильтра
  let showHome = $derived(filter === 'all' && !!home);

  // Стартовый фокус — первая плитка «Продолжить просмотр» (для навигации с пульта)
  let continueRow = $state<HTMLElement | null>(null);
  let initialFocusDone = false;

  $effect(() => {
    if (initialFocusDone || !continueRow) return;
    const first = continueRow.querySelector<HTMLElement>('button, a[href], [tabindex]:not([tabindex="-1"])');
    if (!first) return;
    initialFocusDone = true;
    first.focus();
  });

</script>

{#if loading}
  <ScreenMessage text="Загрузка..." />
{:else}
  <div class="page">
    <AppHeader>
      <h1 class="header__logo">Aurora</h1>
      <div class="header__actions">
        <button onclick={handleScan} class="btn">Обновить</button>
        <button onclick={() => (pairOpen = true)} class="btn">Подключить ТВ</button>
        <button onclick={() => auth.logout()} class="btn">Выйти</button>
      </div>
    </AppHeader>

    {#if pairOpen}
      <PairTvModal onClose={() => (pairOpen = false)} />
    {/if}

    <!-- Фильтры -->
    <div class="container toolbar">
      <div class="toolbar__filters">
        <button onclick={() => (filter = 'all')} class="btn" class:btn--primary={filter === 'all'}>Все</button>
        <button onclick={() => (filter = 'movie')} class="btn" class:btn--primary={filter === 'movie'}>Фильмы</button>
        <button onclick={() => (filter = 'tvseries')} class="btn" class:btn--primary={filter === 'tvseries'}>
          Сериалы
        </button>
      </div>
    </div>

    {#if showHome && home && home.continue_watching.length > 0}
      <div class="container sections">
        <Section title="Продолжить просмотр">
          <Row bind:element={continueRow}>
            {#each home.continue_watching as item (item.id)}
              <ContinueCard {item} />
            {/each}
          </Row>
        </Section>
      </div>
    {/if}

    <!-- Сетка медиа -->
    <div class="container library">
      <select class="select library__sort" bind:value={sort} aria-label="Сортировка">
        <option value="newest">Сначала новые</option>
        <option value="title">По алфавиту</option>
        <option value="title-desc">По алфавиту (Я–А)</option>
      </select>
      {#if filteredAndSortedMedia.length === 0}
        <div class="empty">Медиатека пуста. Нажмите «Обновить».</div>
      {:else}
        <div class="media-grid">
          {#each filteredAndSortedMedia as item (item.id)}
            <MediaCard media={item} progress={progressMap[item.id]} />
          {/each}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .header__logo {
    font-size: var(--text-2xl);
    font-weight: var(--weight-bold);
  }

  .header__actions {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .toolbar {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding-top: var(--space-6);
  }

  .toolbar__filters {
    display: flex;
    gap: var(--space-2);
  }

  @media (min-width: 640px) {
    .toolbar {
      flex-direction: row;
      justify-content: space-between;
    }
  }

  .sections {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding-top: var(--space-6);
  }

  .library {
    padding-block: var(--space-6);
  }

  .library__sort {
    margin-bottom: var(--space-4);
  }

  .empty {
    padding-block: var(--space-12);
    text-align: center;
    color: var(--text-muted);
    font-size: var(--text-lg);
  }
</style>
