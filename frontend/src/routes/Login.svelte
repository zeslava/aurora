<script lang="ts">
  import { auth } from '../lib/auth.svelte';
  import { navigate } from '../lib/router.svelte';
  import TvPairing from '../components/TvPairing.svelte';

  type Mode = 'login' | 'register' | 'tv';

  let mode = $state<Mode>('login');
  let username = $state('');
  let password = $state('');
  let confirmPassword = $state('');
  let error = $state('');
  let loading = $state(false);

  let isRegister = $derived(mode === 'register');

  // Из домашней сети вход уже выполнил auth при запуске — сюда попадают только снаружи
  $effect(() => {
    if (auth.isAuthenticated) navigate('/library', { replace: true });
  });

  const switchMode = (next: Mode) => {
    mode = next;
    error = '';
    password = '';
    confirmPassword = '';
  };

  const handleSubmit = async (e: SubmitEvent) => {
    e.preventDefault();
    error = '';

    if (isRegister) {
      if (password !== confirmPassword) {
        error = 'Пароли не совпадают';
        return;
      }
      if (password.length < 6) {
        error = 'Пароль должен быть не менее 6 символов';
        return;
      }
    }

    loading = true;

    try {
      if (isRegister) {
        await auth.register({ username, password });
      } else {
        await auth.login({ username, password });
      }
      navigate('/library', { replace: true });
    } catch (err: any) {
      error =
        err.response?.data?.message ||
        (isRegister
          ? 'Ошибка регистрации. Возможно, пользователь уже существует.'
          : 'Ошибка входа. Проверьте имя пользователя и пароль.');
    } finally {
      loading = false;
    }
  };

</script>

<div class="auth-screen">
  <div class="auth-card">
    <div>
      <h2 class="auth-title">Aurora Медиасервер</h2>
      <p class="auth-subtitle">
        {mode === 'register'
          ? 'Создайте новый аккаунт'
          : mode === 'tv'
            ? 'Войдите с телевизора без пароля'
            : 'Войдите в свой аккаунт'}
      </p>
    </div>

    <div class="tabs">
      <button type="button" onclick={() => switchMode('login')} class="tab" class:tab--active={mode === 'login'}>
        Вход
      </button>
      <button type="button" onclick={() => switchMode('register')} class="tab" class:tab--active={mode === 'register'}>
        Регистрация
      </button>
      <button type="button" onclick={() => switchMode('tv')} class="tab" class:tab--active={mode === 'tv'}>
        ТВ
      </button>
    </div>

    {#if mode === 'tv'}
      <TvPairing
        onAuthenticated={(response) => {
          auth.loginWithResponse(response);
          navigate('/library', { replace: true });
        }}
      />
    {:else}
      <form class="auth-form" onsubmit={handleSubmit}>
        {#if error}
          <div class="alert">{error}</div>
        {/if}
        <div class="fields">
          <label for="username" class="visually-hidden">Имя пользователя</label>
          <input
            id="username"
            name="username"
            type="text"
            required
            class="input auth-input"
            placeholder="Имя пользователя"
            bind:value={username}
          />

          <label for="password" class="visually-hidden">Пароль</label>
          <input
            id="password"
            name="password"
            type="password"
            required
            class="input auth-input"
            placeholder="Пароль"
            bind:value={password}
          />

          {#if isRegister}
            <label for="confirmPassword" class="visually-hidden">Подтвердите пароль</label>
            <input
              id="confirmPassword"
              name="confirmPassword"
              type="password"
              required
              class="input auth-input"
              placeholder="Подтвердите пароль"
              bind:value={confirmPassword}
            />
          {/if}
        </div>

        <button type="submit" disabled={loading} class="btn btn--primary auth-submit">
          {loading
            ? isRegister
              ? 'Регистрация...'
              : 'Вход...'
            : isRegister
              ? 'Зарегистрироваться'
              : 'Войти'}
        </button>
      </form>
    {/if}
  </div>
</div>

<style>
  .tabs {
    display: flex;
    padding: var(--space-1);
    border-radius: var(--radius);
    background-color: var(--surface);
  }

  .tab {
    flex: 1;
    padding: var(--space-2) var(--space-4);
    border: none;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--text-muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    cursor: pointer;
    transition: background-color var(--transition), color var(--transition);
  }

  .tab:hover {
    color: var(--text);
  }

  .tab--active {
    background-color: var(--accent-strong);
    color: var(--text);
  }

  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  @media (min-width: 1024px) {
    .tab {
      padding-block: var(--space-4);
      font-size: var(--text-xl);
    }
  }
</style>
