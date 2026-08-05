<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** true — шапка лежит поверх кадра (страницы деталей): без подложки и границы */
    transparent?: boolean;
    children: Snippet;
  }

  let { transparent = false, children }: Props = $props();
</script>

<header class="app-header" class:app-header--transparent={transparent}>
  <div class="container header__inner">
    {@render children()}
  </div>
</header>
