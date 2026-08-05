<script lang="ts">
  import PosterPlaceholder from './PosterPlaceholder.svelte';
  import { apiClient } from '../api/client';

  interface Props {
    /** Относительный путь обложки с бэкенда; null — рисуем плейсхолдер */
    poster?: string | null;
    title: string;
    icon?: string;
  }

  let { poster = null, title, icon = '🎬' }: Props = $props();

  // Битую или ещё не скачанную картинку заменяем плейсхолдером
  let failed = $state(false);
  let url = $derived(failed ? null : apiClient.getImageUrl(poster));
  $effect(() => {
    poster;
    failed = false;
  });
</script>

{#if url}
  <img class="poster__image" src={url} alt={title} loading="lazy" onerror={() => (failed = true)} />
{:else}
  <PosterPlaceholder {title} {icon} />
{/if}

<style>
  .poster__image {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
</style>
