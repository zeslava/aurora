// Что этот клиент декодирует сам. Уходит в query плейлиста: сервер отдаёт HEVC как есть
// тому, кто его играет, и кодирует H.264 не выше экрана клиента.
// См. docs/decisions/0012-client-capabilities.md.

// Те же источники MSE, что проверяет Hls.isSupported(): без них плеер идёт в нативный HLS,
// а тот (Safari) HEVC в MPEG-TS не принимает — такому клиенту возможности не сообщаем.
type MediaSourceLike = { isTypeSupported(type: string): boolean };

function mediaSource(): MediaSourceLike | undefined {
  const w = window as unknown as Record<string, MediaSourceLike | undefined>;
  return w.ManagedMediaSource ?? w.MediaSource ?? w.WebKitMediaSource;
}

// Level 5.1 — 4K@30/60 у WEB-DL и BDRip; ниже уровень спрашивать бессмысленно.
const HEVC_MAIN = 'video/mp4; codecs="hvc1.1.6.L153.B0"';
const HEVC_MAIN10 = 'video/mp4; codecs="hvc1.2.4.L153.B0"';

function supports(ms: MediaSourceLike, type: string): boolean {
  try {
    return ms.isTypeSupported(type);
  } catch {
    return false;
  }
}

let cached: string | null = null;

// Query вида `caps=hevc,hevc10,hdr&h=2160`; пустая строка — клиент о себе молчит.
export function clientCapsQuery(): string {
  if (cached !== null) return cached;

  const ms = mediaSource();
  if (!ms) return (cached = '');

  const caps: string[] = [];
  if (supports(ms, HEVC_MAIN)) caps.push('hevc');
  if (supports(ms, HEVC_MAIN10)) caps.push('hevc10');
  // video-dynamic-range — у телевизоров, где графика SDR, а видеослой HDR.
  if (
    matchMedia('(dynamic-range: high)').matches ||
    matchMedia('(video-dynamic-range: high)').matches
  ) {
    caps.push('hdr');
  }

  // Короткая сторона экрана в физических пикселях: у телефона в портрете это ширина.
  const dpr = window.devicePixelRatio || 1;
  const height = Math.round(Math.min(screen.width, screen.height) * dpr);

  return (cached = `caps=${caps.join(',')}&h=${height}`);
}
