<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    onClose: () => void;
    children: Snippet;
  }

  let { title, onClose, children }: Props = $props();

  let card = $state<HTMLElement | null>(null);

  // Фокус внутрь карточки: иначе на ТВ D-pad продолжает ходить по странице
  // под модалкой, а вернуть его некуда. Возвращаем на прежний элемент при закрытии.
  $effect(() => {
    const opener = document.activeElement as HTMLElement | null;
    card?.querySelector<HTMLElement>('input, button, [tabindex]:not([tabindex="-1"])')?.focus();
    return () => opener?.focus();
  });
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape') onClose();
  }}
/>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onClose}>
  <div
    bind:this={card}
    class="modal"
    tabindex="-1"
    role="dialog"
    aria-modal="true"
    aria-label={title}
    onclick={(e) => e.stopPropagation()}
  >
    <h2 class="modal__title">{title}</h2>
    {@render children()}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: var(--z-modal);
    display: flex;
    align-items: center;
    justify-content: center;
    padding-inline: var(--space-4);
    background-color: var(--scrim-strong);
  }

  .modal {
    width: 100%;
    max-width: 24rem;
    padding: var(--space-6);
    border-radius: var(--radius);
    background-color: var(--surface);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .modal__title {
    font-size: var(--text-xl);
    font-weight: var(--weight-bold);
  }
</style>
