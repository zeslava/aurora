import { apiClient } from '../api/client';

// Крупный интерфейс для просмотра с дивана.
// 4K-телевизор в webOS-браузере отдаёт CSS-viewport 1920px, а не 3840,
// поэтому по одной ширине его не отличить от десктопного монитора —
// добавляем к ширине грубый указатель (пульт), а упакованное приложение
// считаем ТВ безусловно.
const TV_QUERY = '(min-width: 1900px) and (pointer: coarse)';

// Маркер в User-Agent от Android TV-оболочки (android/, MainActivity)
const ANDROID_TV_SHELL = navigator.userAgent.includes('AuroraTV');

export function initTvMode(): () => void {
  const mq = window.matchMedia(TV_QUERY);

  // WebView на Android TV отдаёт CSS-ширину экран/density (960px на 1080p) —
  // фиксируем 1920, чтобы брейкпоинты и шкала совпадали с webOS
  if (ANDROID_TV_SHELL) {
    document.querySelector('meta[name="viewport"]')?.setAttribute('content', 'width=1920');
  }

  const apply = () => {
    const isTv = apiClient.isPackagedApp() || ANDROID_TV_SHELL || mq.matches;
    if (isTv) {
      document.documentElement.dataset.tv = '';
    } else {
      delete document.documentElement.dataset.tv;
    }
  };

  apply();
  mq.addEventListener('change', apply);

  return () => {
    mq.removeEventListener('change', apply);
    delete document.documentElement.dataset.tv;
  };
}
