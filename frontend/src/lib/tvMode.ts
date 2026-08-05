import { apiClient } from '../api/client';

// Крупный интерфейс для просмотра с дивана.
// 4K-телевизор в webOS-браузере отдаёт CSS-viewport 1920px, а не 3840,
// поэтому по одной ширине его не отличить от десктопного монитора —
// добавляем к ширине грубый указатель (пульт), а упакованное приложение
// считаем ТВ безусловно.
const TV_QUERY = '(min-width: 1900px) and (pointer: coarse)';

export function initTvMode(): () => void {
  const mq = window.matchMedia(TV_QUERY);

  const apply = () => {
    const isTv = apiClient.isPackagedApp() || mq.matches;
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
