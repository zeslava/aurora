<script lang="ts">
  import { apiClient } from '../api/client';

  interface Props {
    seasons: number[];
    selectedSeason: number;
    onSelectSeason: (season: number) => void;
    /** URL обложки по номеру сезона */
    posters?: Record<string, string>;
  }

  let { seasons, selectedSeason, onSelectSeason, posters = {} }: Props = $props();

  // Обложки может не быть (нет на TMDB, битый файл в кэше) — прячем картинку,
  // кнопка остаётся обычной текстовой
  let failed = $state<Record<number, boolean>>({});

  const posterUrl = (season: number) =>
    failed[season] ? null : apiClient.getImageUrl(posters[season.toString()]);
</script>

<div class="seasons">
  {#each seasons as season (season)}
    {@const url = posterUrl(season)}
    <button
      onclick={() => onSelectSeason(season)}
      class="btn"
      class:btn--primary={season === selectedSeason}
    >
      {#if url}
        <img
          class="seasons__poster"
          src={url}
          alt=""
          loading="lazy"
          onerror={() => (failed[season] = true)}
        />
      {/if}
      Сезон {season}
    </button>
  {/each}
</div>

<style>
  .seasons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  .btn {
    font-weight: var(--weight-semibold);
    gap: var(--space-2);
    height: auto;
    align-items: center;
    overflow: hidden;
  }

  .seasons__poster {
    flex-shrink: 0;
    height: 1.75rem;
    aspect-ratio: 2 / 3;
    object-fit: cover;
    border-radius: var(--radius-sm);
  }
</style>
