import { apiClient } from '../api/client';

// В упакованном приложении (file://) путь страницы — не '/', поэтому
// history-роутинг не находит маршрутов; используем #-фрагмент
const useHash = apiClient.isPackagedApp();

export type RouteName = 'login' | 'library' | 'media' | 'series' | 'watch' | 'notfound';

const PATTERNS: { name: RouteName; parts: string[] }[] = [
  { name: 'login', parts: ['login'] },
  { name: 'library', parts: ['library'] },
  { name: 'media', parts: ['media', ':id'] },
  { name: 'series', parts: ['series', ':id'] },
  { name: 'watch', parts: ['watch', ':id', ':season', ':episode'] },
  { name: 'watch', parts: ['watch', ':id'] },
];

const segments = (path: string): string[] =>
  path.split('/').filter((s) => s.length > 0);

const match = (path: string): { name: RouteName; params: Record<string, string> } => {
  const segs = segments(path);
  for (const pattern of PATTERNS) {
    if (pattern.parts.length !== segs.length) continue;
    const params: Record<string, string> = {};
    let ok = true;
    for (let i = 0; i < segs.length; i++) {
      const part = pattern.parts[i];
      if (part.startsWith(':')) params[part.slice(1)] = decodeURIComponent(segs[i]);
      else if (part !== segs[i]) {
        ok = false;
        break;
      }
    }
    if (ok) return { name: pattern.name, params };
  }
  return { name: 'notfound', params: {} };
};

const currentPath = (): string => {
  if (useHash) return location.hash.replace(/^#/, '') || '/';
  return location.pathname || '/';
};

const isWatch = (path: string) => segments(path)[0] === 'watch';

class Router {
  path = $state(currentPath());
  route = $derived(match(this.path));
  // Откуда пришли на текущий экран — переходы между сериями внутри плеера
  // не сбрасывают исходную страницу
  previous = $state<string | null>(null);

  constructor() {
    const sync = () => {
      this.path = currentPath();
    };
    window.addEventListener(useHash ? 'hashchange' : 'popstate', sync);
  }

  navigate(to: string, options: { replace?: boolean } = {}) {
    if (to === this.path) return;
    if (!(isWatch(this.path) && isWatch(to))) this.previous = this.path;
    if (useHash) {
      const url = `#${to}`;
      if (options.replace) location.replace(url);
      else location.hash = to;
    } else if (options.replace) {
      history.replaceState(null, '', to);
    } else {
      history.pushState(null, '', to);
    }
    this.path = to;
  }

  back() {
    history.back();
  }
}

export const router = new Router();

export const navigate = (to: string, options?: { replace?: boolean }) =>
  router.navigate(to, options);

// Экшен для <a use:link href="/library">: перехватывает клик, не перезагружая страницу
export const link = (node: HTMLAnchorElement) => {
  const onClick = (e: MouseEvent) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
    const to = node.getAttribute('href');
    if (!to || !to.startsWith('/')) return;
    e.preventDefault();
    navigate(to);
  };
  node.addEventListener('click', onClick);
  return {
    destroy() {
      node.removeEventListener('click', onClick);
    },
  };
};

// На 401 apiClient сбрасывает токен — возвращаем пользователя на экран входа
// средствами роутера, чтобы не ломать hash-режим жёстким переходом по URL
apiClient.onUnauthorized(() => {
  if (router.path !== '/login') navigate('/login', { replace: true });
});
