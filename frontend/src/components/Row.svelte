<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** Ссылка на контейнер: нужна вызывающему для наведения фокуса на первую плитку */
    element?: HTMLElement | null;
    children: Snippet;
  }

  let { element = $bindable(null), children }: Props = $props();
</script>

<div class="row" bind:this={element}>
  {@render children()}
</div>

<style>
  /* Горизонтальная лента: отступы компенсируют увеличение плитки при фокусе */
  .row {
    display: flex;
    gap: var(--space-4);
    overflow-x: auto;
    padding: var(--space-4) var(--space-3);
    margin: calc(var(--space-2) * -1) calc(var(--space-3) * -1);
  }
</style>
