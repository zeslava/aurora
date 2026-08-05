<script lang="ts">
  import type { Snippet } from 'svelte';
  import Poster from './Poster.svelte';

  interface Props {
    title: string;
    /** Относительный путь обложки с бэкенда; null — рисуем плейсхолдер */
    poster?: string | null;
    subtitle?: string | null;
    badge?: string | null;
    icon?: string;
    actionLabel?: string;
    progressPercent?: number;
    footer?: Snippet;
    /** 'row' — фиксированная ширина для горизонтальной ленты «Продолжить просмотр» */
    variant?: 'grid' | 'row';
    /** 'wide' — кадр 16/9 (основной формат), 'poster' — вертикальный 2/3 */
    aspect?: 'poster' | 'wide';
    onclick: () => void;
  }

  let {
    title,
    poster = null,
    subtitle = null,
    badge = null,
    icon = '🎬',
    actionLabel = 'Смотреть',
    progressPercent = 0,
    footer,
    variant = 'grid',
    aspect = 'wide',
    onclick,
  }: Props = $props();
</script>

<div
  tabindex="0"
  role="button"
  {onclick}
  onkeydown={(e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onclick();
    }
  }}
  class="tile"
  class:tile--row={variant === 'row'}
>
  <div class="tile__poster poster" class:tile__poster--portrait={aspect === 'poster'}>
    {#if badge}
      <div class="tile__badge">{badge}</div>
    {/if}
    <Poster {poster} {title} {icon} />
    {#if progressPercent > 0}
      <div class="progress progress--overlay">
        <div class="progress__bar" style="width: {progressPercent}%"></div>
      </div>
    {/if}
    <div class="tile__hover">
      <span class="tile__action">{actionLabel}</span>
    </div>
  </div>

  <div class="tile__body">
    <h3 class="tile__title">{title}</h3>
    {#if subtitle}<p class="tile__subtitle">{subtitle}</p>{/if}
    {@render footer?.()}
  </div>
</div>

<style>
  .tile {
    position: relative;
    background-color: var(--surface);
    border-radius: var(--radius);
    overflow: hidden;
    cursor: pointer;
    transition: transform var(--transition), box-shadow var(--transition);
  }

  .tile:hover,
  .tile:focus {
    transform: scale(1.05);
    z-index: var(--z-raised);
    box-shadow: var(--shadow-xl);
  }

  .tile--row {
    width: 16rem;
    flex-shrink: 0;
  }

  .tile__poster {
    aspect-ratio: 16 / 9;
    border-radius: 0;
  }

  .tile__poster--portrait {
    aspect-ratio: 2 / 3;
  }

  .tile__badge {
    position: absolute;
    top: var(--space-2);
    right: var(--space-2);
    z-index: var(--z-raised);
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    background-color: var(--scrim-strong);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
  }

  .tile__hover {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background-color: var(--scrim-media);
    opacity: 0;
    transition: opacity var(--transition);
  }

  .tile:hover .tile__hover,
  .tile:focus .tile__hover {
    opacity: 1;
  }

  .tile__action {
    padding: var(--space-2) var(--space-5);
    border-radius: var(--radius);
    background-color: var(--accent-strong);
    font-weight: var(--weight-semibold);
    transform: scale(0.9);
    transition: transform var(--transition);
  }

  .tile:hover .tile__action,
  .tile:focus .tile__action {
    transform: scale(1);
  }

  .tile__body {
    padding: var(--space-3);
  }

  .tile__title,
  .tile__subtitle {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tile__title {
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  .tile__subtitle {
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
</style>
