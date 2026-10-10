const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

type Direction = 'left' | 'right' | 'up' | 'down';

const KEY_TO_DIRECTION: Record<number, Direction> = {
  37: 'left',
  38: 'up',
  39: 'right',
  40: 'down',
};

// webOS: 461 — Back на пульте, 10009 — Back на Tizen
const BACK_KEYS = [461, 10009];

// Скрытые контролы плеера остаются в потоке (у них ненулевые размеры), поэтому
// одних размеров мало: без проверки visibility/opacity фокус уходит на невидимое
const isVisible = (el: HTMLElement): boolean => {
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return false;
  const style = getComputedStyle(el);
  return style.visibility !== 'hidden' && style.opacity !== '0';
};

const collectFocusable = (): HTMLElement[] =>
  Array.from(document.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(isVisible);

const ROW_TOLERANCE = 4;

// Группируем фокусируемые элементы в визуальные «строки» интерфейса:
// элемент относится к текущей строке, если его вертикальный центр попадает
// в её диапазон. Так хедер, тулбар, лента и каждая линия сетки — отдельные строки
const collectRows = (): HTMLElement[][] => {
  const items = collectFocusable()
    .map((el) => ({ el, rect: el.getBoundingClientRect() }))
    .sort((a, b) => a.rect.top - b.rect.top || a.rect.left - b.rect.left);

  const rows: { els: HTMLElement[]; top: number; bottom: number }[] = [];

  for (const { el, rect } of items) {
    const middle = rect.top + rect.height / 2;
    const row = rows[rows.length - 1];

    if (row && middle >= row.top - ROW_TOLERANCE && middle <= row.bottom + ROW_TOLERANCE) {
      row.els.push(el);
      row.top = Math.min(row.top, rect.top);
      row.bottom = Math.max(row.bottom, rect.bottom);
    } else {
      rows.push({ els: [el], top: rect.top, bottom: rect.bottom });
    }
  }

  return rows.map((row) =>
    row.els.sort((a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left)
  );
};

const center = (rect: DOMRect) => ({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });

// Выбираем ближайший элемент в нужном направлении: движение вдоль оси штрафуется
// меньше, чем смещение поперёк — иначе фокус «прыгает» по диагонали через всю сетку
const pickCandidate = (from: HTMLElement, direction: Direction): HTMLElement | null => {
  const fromRect = from.getBoundingClientRect();
  const fromCenter = center(fromRect);

  let best: HTMLElement | null = null;
  let bestScore = Infinity;

  for (const el of collectFocusable()) {
    if (el === from) continue;

    const rect = el.getBoundingClientRect();
    const c = center(rect);
    const dx = c.x - fromCenter.x;
    const dy = c.y - fromCenter.y;

    let along: number;
    let across: number;
    switch (direction) {
      case 'left':
        if (rect.right > fromRect.left + 1) continue;
        along = -dx;
        across = Math.abs(dy);
        break;
      case 'right':
        if (rect.left < fromRect.right - 1) continue;
        along = dx;
        across = Math.abs(dy);
        break;
      case 'up':
        if (rect.bottom > fromRect.top + 1) continue;
        along = -dy;
        across = Math.abs(dx);
        break;
      case 'down':
        if (rect.top < fromRect.bottom - 1) continue;
        along = dy;
        across = Math.abs(dx);
        break;
    }

    const score = along + across * 3;
    if (score < bestScore) {
      bestScore = score;
      best = el;
    }
  }

  return best;
};

const isTextInput = (el: Element | null): boolean => {
  if (!el) return false;
  const tag = el.tagName;
  if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
  if (tag !== 'INPUT') return false;
  const type = (el as HTMLInputElement).type;
  return type !== 'checkbox' && type !== 'radio' && type !== 'button';
};

/**
 * D-pad-навигация для ТВ: стрелки перемещают фокус по геометрии, OK активирует,
 * Back возвращает назад. Не мешает Magic Remote — фокус ставится тем же `focus()`,
 * что и при наведении курсора. Возвращает функцию отписки (для $effect).
 */
export const initSpatialNavigation = (onBack?: () => void) => {
  const handleKeyDown = (e: KeyboardEvent) => {
    const active = document.activeElement as HTMLElement | null;

    if (BACK_KEYS.includes(e.keyCode) || e.key === 'Escape') {
      e.preventDefault();
      if (onBack) onBack();
      else window.history.back();
      return;
    }

    // В плеере и текстовых полях стрелки нужны самим элементам
    if (active?.closest('[data-no-spatial]')) return;

    if (e.keyCode === 13 && active && active.tagName !== 'BUTTON' && active.tagName !== 'A') {
      if (isTextInput(active)) return;
      e.preventDefault();
      active.click();
      return;
    }

    const direction = KEY_TO_DIRECTION[e.keyCode];
    if (!direction) return;
    const horizontal = direction === 'left' || direction === 'right';
    // Закрытый select стрелки влево/вправо не использует — с него можно уйти по строке
    if (horizontal && isTextInput(active) && active?.tagName !== 'SELECT') return;
    // Иначе на краю строки, где фокусу некуда уйти, стрелка листает значения select
    if (horizontal && active?.tagName === 'SELECT') e.preventDefault();

    const focusables = collectFocusable();
    if (focusables.length === 0) return;

    if (!active || active === document.body || !focusables.includes(active)) {
      e.preventDefault();
      focusables[0].focus();
      return;
    }

    // Вверх/вниз — переключение между строками интерфейса, всегда на первый элемент строки
    if (direction === 'up' || direction === 'down') {
      const rows = collectRows();
      const index = rows.findIndex((row) => row.includes(active));

      if (index !== -1) {
        e.preventDefault();
        const target = rows[index + (direction === 'down' ? 1 : -1)];
        if (!target) return;
        target[0].focus();
        target[0].scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' });
        return;
      }
    }

    // Влево/вправо — сначала соседний элемент той же строки: по геометрии далёкий
    // элемент на правом краю тулбара проигрывает ближайшей карточке строкой ниже
    if (horizontal) {
      const row = collectRows().find((r) => r.includes(active));
      const target = row?.[row.indexOf(active) + (direction === 'right' ? 1 : -1)];
      if (target) {
        e.preventDefault();
        target.focus();
        target.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' });
        return;
      }
    }

    const next = pickCandidate(active, direction);
    if (next) {
      e.preventDefault();
      next.focus();
      next.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' });
    }
  };

  window.addEventListener('keydown', handleKeyDown);
  return () => window.removeEventListener('keydown', handleKeyDown);
};
