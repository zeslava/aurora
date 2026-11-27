import type { MediaItem } from '../types';

export function formatMediaYears(
  media: Pick<MediaItem, 'type' | 'year' | 'end_year' | 'ongoing'>
): string | null {
  if (!media.year) return null;
  if (media.type === 'tvseries') {
    if (media.ongoing === true) return `${media.year} — ...`;
    if (media.end_year && media.end_year > media.year) {
      return `${media.year}–${media.end_year}`;
    }
  }
  return String(media.year);
}
