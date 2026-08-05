<script lang="ts">
  import { apiClient } from './api/client';
  import { router, navigate } from './lib/router.svelte';
  import { auth } from './lib/auth.svelte';
  import { initSpatialNavigation } from './lib/spatialNavigation';
  import { initPointerMode } from './lib/pointerMode';
  import { initTvMode } from './lib/tvMode';
  import ServerSetup from './routes/ServerSetup.svelte';
  import Login from './routes/Login.svelte';
  import Library from './routes/Library.svelte';
  import MovieDetail from './routes/MovieDetail.svelte';
  import SeriesDetail from './routes/SeriesDetail.svelte';
  import Watch from './routes/Watch.svelte';

  // В упакованном webOS-приложении сначала спрашиваем адрес сервера.
  // 'checking' — проверяем доступность сохранённого адреса, чтобы неверный
  // адрес вернул на экран ввода, а не «подвесил» приложение
  let gate = $state<'checking' | 'setup' | 'ready'>(
    apiClient.needsServerSetup() ? 'setup' : apiClient.isPackagedApp() ? 'checking' : 'ready'
  );

  $effect(() => {
    if (gate !== 'checking') return;
    apiClient.pingServer().then((ok) => {
      gate = ok ? 'ready' : 'setup';
    });
  });

  // Back на корневом экране закрывает приложение, а не уводит в историю
  // (иначе на ТВ пользователь попадал на экран входа и это выглядело как выход из учётки)
  const handleBack = () => {
    if (router.route.name === 'library' || router.route.name === 'login') {
      exitApp();
      return;
    }
    history.back();
  };

  const exitApp = () => {
    const tizenApp = (window as any).tizen?.application;
    if (tizenApp) {
      tizenApp.getCurrentApplication().exit();
      return;
    }
    window.close();
  };

  $effect(() => initSpatialNavigation(handleBack));
  $effect(() => initPointerMode());
  $effect(() => initTvMode());

  let route = $derived(router.route);

  // Защищённые маршруты: без токена уводим на экран входа
  $effect(() => {
    if (gate !== 'ready' || auth.loading) return;
    if (!auth.isAuthenticated && route.name !== 'login') {
      navigate('/login', { replace: true });
    }
  });

  // Корень и неизвестные пути ведут в библиотеку
  $effect(() => {
    if (gate === 'ready' && route.name === 'notfound') {
      navigate('/library', { replace: true });
    }
  });
</script>

{#if gate === 'checking'}
  <div class="screen-message">Подключение к серверу…</div>
{:else if gate === 'setup'}
  <ServerSetup onConfigured={() => (gate = 'ready')} />
{:else if auth.loading}
  <div class="screen-message">Загрузка...</div>
{:else if route.name === 'login'}
  <Login />
{:else if auth.isAuthenticated}
  {#if route.name === 'library'}
    <Library />
  {:else if route.name === 'media'}
    <MovieDetail id={route.params.id} />
  {:else if route.name === 'series'}
    <SeriesDetail id={route.params.id} />
  {:else if route.name === 'watch'}
    <Watch id={route.params.id} season={route.params.season} episode={route.params.episode} />
  {/if}
{/if}
