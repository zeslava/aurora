<script lang="ts">
  import { apiClient } from '../api/client';
  import type { MediaItem } from '../types';

  interface Props {
    media: MediaItem;
    onupdate: (media: MediaItem) => void;
  }

  let { media, onupdate }: Props = $props();

  let editing = $state(false);
  let value = $state('');
  let saving = $state(false);
  let error = $state('');

  const open = () => {
    value = media.custom_title ?? media.title;
    error = '';
    editing = true;
  };

  const save = async () => {
    if (saving) return;
    saving = true;
    error = '';
    try {
      onupdate(await apiClient.setCustomTitle(media.id, value));
      editing = false;
    } catch (e) {
      console.error('Не удалось сохранить название:', e);
      error = 'Не удалось сохранить название';
    } finally {
      saving = false;
    }
  };

  // Сброс на название, распарсенное из имени файла
  const reset = async () => {
    value = '';
    await save();
  };

  const onkeydown = (e: KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      save();
    }
    if (e.key === 'Escape') editing = false;
  };
</script>

{#if editing}
  <div class="title-editor">
    <!-- svelte-ignore a11y_autofocus -->
    <input
      class="hero__title title-editor__input"
      bind:value
      {onkeydown}
      onblur={save}
      autofocus
      disabled={saving}
      placeholder="Название"
    />
    <div class="title-editor__hint">
      <span>Enter — сохранить, Esc — отмена</span>
      {#if media.custom_title}
        <!-- onmousedown, чтобы сработать раньше blur у поля -->
        <button class="btn--plain" onmousedown={reset} disabled={saving}>Сбросить</button>
      {/if}
    </div>
    {#if error}
      <div class="alert">{error}</div>
    {/if}
  </div>
{:else}
  <h1 class="hero__title title-editor__title">
    {media.title}
    <button
      class="title-editor__pencil"
      onclick={open}
      title="Изменить название"
      aria-label="Изменить название">✎</button
    >
  </h1>
{/if}

<style>
  .title-editor {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    max-width: 32rem;
  }

  .title-editor__input {
    width: 100%;
    padding: 0;
    color: inherit;
    background: transparent;
    border: none;
    border-bottom: 2px solid var(--accent);
    outline: none;
  }

  .title-editor__hint {
    display: flex;
    gap: var(--space-3);
    align-items: center;
    font-size: var(--text-sm);
    color: var(--text-dim);
  }

  .title-editor__title {
    display: inline-flex;
    gap: var(--space-3);
    align-items: center;
  }

  .title-editor__pencil {
    padding: 0;
    font-size: 0.5em;
    line-height: 1;
    color: var(--text-muted);
    cursor: pointer;
    background: none;
    border: none;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .title-editor__title:hover .title-editor__pencil,
  .title-editor__pencil:focus-visible {
    opacity: 1;
  }
</style>
