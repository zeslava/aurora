const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

const D_PAD_KEYS = new Set([13, 37, 38, 39, 40, 461, 10009]);

const setMode = (mode: 'pointer' | 'key') => {
  document.documentElement.dataset.inputMode = mode;
};

/**
 * Magic Remote (курсор LG webOS). Работает параллельно с D-pad-навигацией:
 * движение курсора переводит приложение в pointer-режим и переносит фокус на
 * элемент под курсором — чтобы при переключении обратно на стрелки навигация
 * продолжалась с той же точки. Нажатие стрелок/OK/Back возвращает key-режим.
 * Режим прокинут в `data-input-mode` на <html>, CSS гасит D-pad-обводку под курсором.
 */
export const initPointerMode = () => {
  setMode('key');

  // Курсор двигается непрерывно — троттлим фокусировку до раза в кадр
  let scheduled = false;
  let lastTarget: HTMLElement | null = null;

  const focusUnderCursor = (target: EventTarget | null) => {
    const el = (target as HTMLElement | null)?.closest<HTMLElement>(FOCUSABLE_SELECTOR);
    if (!el || el === lastTarget) return;
    lastTarget = el;
    if (el !== document.activeElement) el.focus({ preventScroll: true });
  };

  const handlePointerMove = (e: MouseEvent) => {
    setMode('pointer');
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      focusUnderCursor(e.target);
    });
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (D_PAD_KEYS.has(e.keyCode)) {
      setMode('key');
      lastTarget = null;
    }
  };

  window.addEventListener('mousemove', handlePointerMove, { passive: true });
  window.addEventListener('keydown', handleKeyDown, true);
  return () => {
    window.removeEventListener('mousemove', handlePointerMove);
    window.removeEventListener('keydown', handleKeyDown, true);
  };
};
