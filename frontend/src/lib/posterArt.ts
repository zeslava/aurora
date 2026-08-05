/** Детерминированная генерация постера-заглушки из названия. */

const STOP_WORDS = new Set([
  'the',
  'a',
  'an',
  'of',
  'and',
  'и',
  'в',
  'на',
  'по',
  'для',
  'с',
]);

/** Инициалы названия: 1–3 значимых слова, либо две первых буквы для односложного. */
export function initialsFor(title: string): string {
  const words = title
    .split(/[\s._\-–—:]+/)
    .map((w) => w.replace(/[^\p{L}\p{N}]/gu, ''))
    // Токены, начинающиеся с цифры (год, «1080p»), — это релиз-мусор, а не название
    .filter((w) => w.length > 0 && !STOP_WORDS.has(w.toLowerCase()) && !/^\d/.test(w));

  if (words.length === 0) {
    // Название целиком числовое («1917») — берём первые символы как есть
    const fallback = title.replace(/[^\p{L}\p{N}]/gu, '');
    return fallback ? fallback.slice(0, 2).toUpperCase() : '?';
  }
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();

  return words
    .slice(0, 3)
    .map((w) => w[0])
    .join('')
    .toUpperCase();
}

/** Оттенок фона (0–359) из djb2-хэша: одно название всегда даёт один цвет. */
export function hueFor(title: string): number {
  let hash = 5381;
  for (let i = 0; i < title.length; i++) {
    hash = ((hash << 5) + hash + title.charCodeAt(i)) | 0;
  }
  return Math.abs(hash) % 360;
}
