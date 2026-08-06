<script lang="ts">
  import PosterPlaceholder from './PosterPlaceholder.svelte';
  import { apiClient } from '../api/client';

  interface Props {
    /** Относительный путь кадра с бэкенда; null — плейсхолдер по названию */
    backdrop?: string | null;
    /** Запасной кадр: показывается, если основного нет или он не загрузился */
    fallback?: string | null;
    title: string;
    icon?: string;
    /** Какую часть кадра держать при обрезке (вертикальный постер — верх) */
    objectPosition?: string;
  }

  let {
    backdrop = null,
    fallback = null,
    title,
    icon = '🎬',
    objectPosition = 'center',
  }: Props = $props();

  // Битую или ещё не скачанную картинку заменяем запасной, а её — плейсхолдером
  let backdropBroken = $state(false);
  let fallbackBroken = $state(false);
  $effect(() => {
    backdrop;
    backdropBroken = false;
  });
  $effect(() => {
    fallback;
    fallbackBroken = false;
  });

  let path = $derived(
    backdrop && !backdropBroken ? backdrop : fallback && !fallbackBroken ? fallback : null
  );
  let src = $derived(apiClient.getImageUrl(path));

  const handleError = () => {
    if (path === backdrop) backdropBroken = true;
    else fallbackBroken = true;
  };

  // Кроссфейд: новый кадр проявляется поверх старого и только после загрузки —
  // иначе при быстром переборе сезонов с пульта мигает пустой фон
  let shown = $state<string | null>(null);
  let loaded = $state(false);
  $effect(() => {
    src;
    loaded = false;
  });
</script>

<!-- Кадр во весь экран: фон страницы деталей, затемнённый градиентом.
     Остаётся на месте при прокрутке контента. -->
<div class="detail-bg" aria-hidden="true">
  {#if shown}
    <img class="detail-bg__layer" src={shown} alt="" style:object-position={objectPosition} />
  {:else if !src}
    <PosterPlaceholder {title} {icon} />
  {/if}

  {#if src && src !== shown}
    {#key src}
      <img
        class="detail-bg__layer detail-bg__layer--next"
        class:detail-bg__layer--loaded={loaded}
        {src}
        alt=""
        style:object-position={objectPosition}
        onload={() => (loaded = true)}
        onerror={handleError}
        ontransitionend={() => (shown = src)}
      />
    {/key}
  {/if}
</div>

<style>
  .detail-bg__layer {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .detail-bg__layer--next {
    opacity: 0;
    transition: opacity var(--transition-slow);
  }

  .detail-bg__layer--loaded {
    opacity: 1;
  }
</style>
