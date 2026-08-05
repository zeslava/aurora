<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    /** true — увеличенный отступ под заголовком (заголовок над крупной сеткой) */
    spaced?: boolean;
    children: Snippet;
  }

  let { title, spaced = false, children }: Props = $props();
</script>

<section>
  <h2 class="section__title" class:section__title--spaced={spaced}>{title}</h2>
  {@render children()}
</section>

<style>
  .section__title {
    margin-bottom: var(--space-3);
    font-size: var(--text-xl);
    font-weight: var(--weight-bold);
  }

  .section__title--spaced {
    margin-bottom: var(--space-4);
  }
</style>
