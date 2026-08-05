<script lang="ts">
  import { apiClient } from '../api/client';

  // Экран ввода адреса сервера. Нужен в упакованном webOS-приложении (.ipk),
  // где нет своего origin: один и тот же билд работает с любым сервером Aurora.
  interface Props {
    onConfigured: () => void;
  }

  let { onConfigured }: Props = $props();

  let url = $state(apiClient.getServerUrl() || 'http://');
  let error = $state('');
  let loading = $state(false);

  const handleSubmit = async (e: SubmitEvent) => {
    e.preventDefault();
    error = '';

    const trimmed = url.trim();
    if (!/^https?:\/\/.+/.test(trimmed)) {
      error = 'Введите адрес вида http://192.168.1.10:4000';
      return;
    }

    // Запоминаем адрес сразу, даже если он неверный — чтобы при следующем
    // запуске поле было предзаполнено и не пришлось вводить заново
    apiClient.setServerUrl(trimmed);

    loading = true;
    try {
      // Проверка доступности лишь решает, пускать дальше или остаться на экране
      const ctrl = new AbortController();
      const t = setTimeout(() => ctrl.abort(), 5000);
      try {
        await fetch(`${apiClient.getServerUrl()}/api/libraries`, { signal: ctrl.signal });
      } finally {
        clearTimeout(t);
      }
      onConfigured();
    } catch {
      error = 'Сервер недоступен. Проверьте адрес и что он запущен. Адрес сохранён.';
    } finally {
      loading = false;
    }
  };
</script>

<div class="auth-screen">
  <div class="auth-card">
    <div>
      <h1 class="auth-title">Aurora</h1>
      <p class="auth-subtitle">Укажите адрес сервера</p>
    </div>
    <form onsubmit={handleSubmit} class="auth-form">
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="url"
        bind:value={url}
        placeholder="http://192.168.1.10:4000"
        autofocus
        class="input auth-input"
      />
      {#if error}<p class="text-error">{error}</p>{/if}
      <button type="submit" disabled={loading} class="btn btn--primary auth-submit">
        {loading ? 'Проверка…' : 'Подключиться'}
      </button>
    </form>
  </div>
</div>
