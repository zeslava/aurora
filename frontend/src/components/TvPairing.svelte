<script lang="ts">
  import { apiClient } from '../api/client';
  import type { AuthResponse } from '../types';

  interface Props {
    onAuthenticated: (response: AuthResponse) => void;
  }

  let { onAuthenticated }: Props = $props();

  let code = $state<string | null>(null);
  let error = $state('');

  $effect(() => {
    let active = true;
    let pollTimer: number | undefined;

    const startPairing = async () => {
      try {
        const { pairing_id, code: newCode } = await apiClient.pairStart();
        if (!active) return;
        code = newCode;
        error = '';

        pollTimer = window.setInterval(async () => {
          try {
            const response = await apiClient.pairPoll(pairing_id);
            if (response && active) {
              window.clearInterval(pollTimer);
              onAuthenticated(response);
            }
          } catch {
            // 404 — код истёк, перезапускаем сессию
            if (active) {
              window.clearInterval(pollTimer);
              code = null;
              startPairing();
            }
          }
        }, 3000);
      } catch {
        if (active) error = 'Не удалось получить код. Попробуйте позже.';
      }
    };

    startPairing();

    return () => {
      active = false;
      if (pollTimer) window.clearInterval(pollTimer);
    };
  });
</script>

<div class="pairing">
  {#if error}
    <div class="alert">{error}</div>
  {/if}
  <p class="pairing__hint">
    Откройте Aurora на телефоне или компьютере, войдите и введите этот код в разделе «Подключить ТВ».
  </p>
  <div class="pairing__code">{code ?? '······'}</div>
  <p class="pairing__note">Код действителен 5 минут. Ожидание подтверждения…</p>
</div>

<style>
  .pairing {
    margin-top: var(--space-8);
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    text-align: center;
  }

  .pairing__hint {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }

  .pairing__code {
    padding-block: var(--space-6);
    border-radius: var(--radius);
    background-color: var(--surface);
    font-family: var(--font-mono);
    font-size: var(--text-5xl);
    font-weight: var(--weight-bold);
    letter-spacing: 0.3em;
  }

  .pairing__note {
    color: var(--text-dim);
    font-size: var(--text-xs);
  }

  /* Крупнее на ТВ — экран смотрят с расстояния */
  @media (min-width: 1024px) {
    .pairing__hint {
      font-size: var(--text-2xl);
    }

    .pairing__code {
      padding-block: var(--space-12);
      font-size: var(--text-7xl);
    }

    .pairing__note {
      font-size: var(--text-lg);
    }
  }
</style>
