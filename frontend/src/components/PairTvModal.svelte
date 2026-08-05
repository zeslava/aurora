<script lang="ts">
  import { apiClient } from '../api/client';
  import Modal from './Modal.svelte';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let code = $state('');
  let status = $state<'idle' | 'loading' | 'done' | 'error'>('idle');
  let message = $state('');

  const handleConfirm = async (e: SubmitEvent) => {
    e.preventDefault();
    status = 'loading';
    message = '';
    try {
      await apiClient.pairConfirm(code.trim());
      status = 'done';
      message = 'Готово! Телевизор войдёт автоматически.';
    } catch (err: any) {
      status = 'error';
      message = err.response?.data?.message || 'Не удалось подтвердить код.';
    }
  };
</script>

<Modal title="Подключить телевизор" {onClose}>
  <p class="modal__hint">Введите 6-значный код, показанный на экране телевизора.</p>
  <form onsubmit={handleConfirm} class="modal__form">
    <input
      type="text"
      inputmode="numeric"
      maxlength="6"
      value={code}
      oninput={(e) => (code = e.currentTarget.value.replace(/\D/g, ''))}
      placeholder="000000"
      class="input code-input"
    />
    {#if message}
      <p class={status === 'done' ? 'text-success' : 'text-error'}>{message}</p>
    {/if}
    <div class="modal__actions">
      <button
        type="submit"
        disabled={code.length !== 6 || status === 'loading' || status === 'done'}
        class="btn btn--primary modal__submit"
      >
        {status === 'loading' ? 'Подтверждение...' : 'Подтвердить'}
      </button>
      <button type="button" onclick={onClose} class="btn">Закрыть</button>
    </div>
  </form>
</Modal>

<style>
  .modal__hint {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }

  .modal__form {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .code-input {
    padding: var(--space-3);
    background-color: var(--bg);
    text-align: center;
    font-family: var(--font-mono);
    font-size: var(--text-3xl);
    letter-spacing: 0.3em;
  }

  .modal__actions {
    display: flex;
    gap: var(--space-2);
  }

  .modal__submit {
    flex: 1;
  }
</style>
