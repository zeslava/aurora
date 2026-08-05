import type { PlaybackState } from '../types';

// Насколько близко к концу сохранённая позиция считается «досмотрено»
const TAIL_THRESHOLD = 60;

// Позиция в хвосте файла: досматривать нечего, резюм с неё молча уведёт на следующую серию
export const isAtTail = (
  progress: PlaybackState | null | undefined,
  duration: number | null | undefined
): boolean => {
  if (!progress) return false;
  const total = duration || 0;
  return total > 0 && progress.position >= total - TAIL_THRESHOLD;
};

export const isWatchedToEnd = (
  progress: PlaybackState | null | undefined,
  duration: number | null | undefined
): boolean => {
  if (!progress) return false;
  if (progress.completed) return true;
  return isAtTail(progress, duration);
};
