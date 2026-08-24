<script lang="ts">
  import Hls from 'hls.js';
  import { untrack, type Snippet } from 'svelte';
  import { apiClient } from '../api/client';
  import type { AudioTrack } from '../types';

  interface Props {
    src: string;
    mediaId: string;
    duration?: number;
    onTimeUpdate?: (currentTime: number) => void;
    onEnded?: () => void;
    autoplay?: boolean;
    startTime?: number;
    audioCodec?: string; // Аудио кодек для диагностики
    audioTracks?: AudioTrack[]; // Аудиодорожки файла (переводы) — для выбора в плеере
    transcoded?: boolean; // Используется ли транскодированный поток
    onSrcChange?: (newSrc: string) => void; // Callback для обновления src при перемотке
    onNextEpisode?: () => void; // Переход к следующему эпизоду (undefined — если следующего нет)
    watched?: boolean; // Уже просмотрено — play показывается как «заново»
    overlay?: Snippet; // Контент поверх видео (заголовок, кнопка «Назад») — скрывается вместе с контролами
  }

  let {
    src,
    mediaId,
    duration,
    onTimeUpdate,
    onEnded,
    autoplay = false,
    startTime = 0,
    audioCodec,
    audioTracks = [],
    transcoded = false,
    onSrcChange,
    onNextEpisode,
    watched = false,
    overlay,
  }: Props = $props();

  const audioTrackKey = (id: string) => `aurora.audioTrack.${id}`;

  /// Выбор перевода запоминается на файл: вернувшись к фильму, зритель ждёт тот же голос.
  const storedAudioIndex = (id: string): number => {
    try {
      const raw = localStorage.getItem(audioTrackKey(id));
      const parsed = raw === null ? NaN : Number(raw);
      return Number.isInteger(parsed) && parsed >= 0 ? parsed : 0;
    } catch {
      return 0;
    }
  };

  /// Подпись дорожки: язык и число каналов различают переводы лучше, чем номер.
  const audioTrackLabel = (track: AudioTrack): string => {
    const parts = [`Дорожка ${track.index + 1}`];
    if (track.language) parts.push(track.language.toUpperCase());
    if (track.codec) parts.push(track.codec.toUpperCase());
    if (track.channels) parts.push(track.channels === 6 ? '5.1' : `${track.channels}.0`);
    if (track.title) parts.push(track.title);
    return parts.join(' · ');
  };

  // За сколько секунд до конца показывать оверлей «Следующий эпизод»
  const NEXT_EP_THRESHOLD = 15;

  let videoEl = $state<HTMLVideoElement | null>(null);
  let playerEl = $state<HTMLElement | null>(null);
  // Невидимый держатель фокуса: пока смотрим, фокус «припаркован» здесь, и
  // spatial-навигация не уводит его на скрытые контролы (data-no-spatial)
  let idleEl = $state<HTMLElement | null>(null);
  let seekEl = $state<HTMLInputElement | null>(null);
  let playEl = $state<HTMLButtonElement | null>(null);
  // Пропсы ниже используются только как начальные значения — как в React-версии
  /* svelte-ignore state_referenced_locally */
  let isPlaying = $state(autoplay);
  /* svelte-ignore state_referenced_locally */
  let currentTime = $state(startTime);
  let isFullscreen = $state(false);
  let showControls = $state(true);
  let audioMenuOpen = $state(false);

  // Снимок id: при размонтировании родитель уже мог обнулить своё состояние,
  // а teardown-эффекты читают пропсы лениво (геттеры) — отсюда падение на null
  let currentMediaId = untrack(() => mediaId);
  $effect(() => {
    currentMediaId = mediaId;
  });

  let lastSaveTime = 0;
  let controlsTimeout: number | undefined;
  let hls: Hls | null = null;
  let fatalCount = 0;
  let playbackError = $state<string | null>(null);
  // Для транскодированного (не-HLS) потока сервер начинает с указанной позиции —
  // нужно отслеживать смещение (offset) для корректного отображения времени
  /* svelte-ignore state_referenced_locally */
  let streamOffset = $state(startTime);

  let isHls = $derived(src.includes('.m3u8'));
  let offsetActive = $derived(transcoded && !isHls);
  // Выбранная дорожка живёт в плеере: сервер отдаёт её в плейлисте, поэтому смена
  // дорожки — это смена источника, а не переключение внутри готового потока.
  let audioIndex = $state(untrack(() => storedAudioIndex(mediaId)));
  // Позиция, с которой пересоздаётся hls.js после смены дорожки. `startTime` для этого
  // не годится: он остаётся стартовым на весь просмотр.
  let resumePosition: number | null = null;

  let hlsSrc = $derived(isHls ? apiClient.getHlsUrl(mediaId, audioIndex) : src);

  // HLS-поток: hls.js (MSE) везде, где есть MSE, — включая webOS/Tizen. Нативный HLS
  // остаётся только фолбэком для браузеров без MSE (старая Safari на iOS).
  // Раньше приоритет был обратный, и телевизор уходил в нативную ветку: она ставит позицию
  // единственным seek'ом уже после старта с нуля и не умеет восстанавливаться после ошибок,
  // из-за чего «продолжить с середины» вставало намертво и молча доезжало до конца файла.
  // Один движок на всех платформах — один контракт стартовой позиции и одна обработка ошибок.
  // URL плейлиста постоянен на весь просмотр — сервер не знает позицию,
  // ей владеет плеер (currentTime/seek), поэтому эффект зависит только от src.
  $effect(() => {
    const video = videoEl;
    const source = hlsSrc;
    if (!video || !source || !isHls) return;

    if (hls) {
      hls.destroy();
      hls = null;
    }
    fatalCount = 0;
    playbackError = null;
    completedSent = false;

    const playIfNeeded = () => {
      if (isPlaying || autoplay) video.play().catch(() => {});
    };

    // Нативный HLS — только там, где MSE нет вообще: hls.js предпочтительнее даже при
    // наличии нативной поддержки (см. комментарий к эффекту).
    if (!Hls.isSupported() && video.canPlayType('application/vnd.apple.mpegurl')) {
      // Нативный HLS: аналога startPosition нет, позиция ставится
      // единственным seek'ом сразу после метаданных — и ровно один раз, иначе
      // повторные seek'и гоняют плеер по таймлайну.
      const startNative = untrack(() => startTime);
      // Диагностика нативной ветки: консоль доступна только через `ares-inspect`, и без
      // этих строк с ТВ не видно ничего — ни куда встал playhead, ни что в буфере.
      const ranges = () =>
        Array.from({ length: video.buffered.length }, (_, i) =>
          `${video.buffered.start(i).toFixed(1)}..${video.buffered.end(i).toFixed(1)}`
        ).join(',') || 'пусто';
      const trace = (event: string) =>
        console.log(
          `нативный HLS: ${event}, currentTime=${video.currentTime.toFixed(1)}, ` +
            `readyState=${video.readyState}, буфер=[${ranges()}]`
        );
      for (const event of ['seeking', 'seeked', 'waiting', 'stalled', 'playing', 'ended']) {
        video.addEventListener(event, () => trace(event));
      }

      video.src = source;
      video.load();
      video.addEventListener(
        'loadedmetadata',
        () => {
          console.log(
            `нативный HLS: метаданные, duration=${video.duration}, стартовая позиция=${startNative}`
          );
          if (startNative > 0) video.currentTime = startNative;
          playIfNeeded();
        },
        { once: true }
      );
    } else if (Hls.isSupported()) {
      // Пересоздание Hls с нуля — единственный способ перечитать плейлист: VOD-манифест
      // (с #EXT-X-ENDLIST) hls.js загружает один раз и больше не запрашивает. Позиция
      // передаётся аргументом и берётся ровно один раз на создание — повторная установка
      // гоняет плеер по таймлайну (см. коммит про «разгон плеера при возобновлении»).
      const createHls = (position: number) => {
        // Плейлист отдаётся, как только ffmpeg записал первый сегмент, но запуск
        // кодирования может занять несколько секунд — стандартный 10-секундный
        // таймаут hls.js мал и приводит к бесконечным ретраям.
        hls = new Hls({
          manifestLoadingTimeOut: 60000,
          manifestLoadingMaxRetry: 2,
          manifestLoadingRetryDelay: 2000,
          startPosition: position,
        });
        // Без этого обработчика фатальные ошибки hls.js проявляются как «видео просто встало»
        hls.on(Hls.Events.ERROR, (_e, data) => {
          console.error('hls.js:', data.type, data.details, data.fatal ? '(fatal)' : '', data);
          if (!data.fatal || !hls) return;
          fatalCount++;
          if (fatalCount > 3) {
            hls.destroy();
            hls = null;
            playbackError = 'Не удалось воспроизвести поток';
            return;
          }
          // 404 на фрагменте почти всегда означает, что профиль потока на сервере сменился
          // и манифест на руках устарел (сервер помечает сегменты `?p=<профиль>` и отвечает
          // 404 на чужой маркер). startLoad() тут бесполезен — он повторяет запросы по тем же
          // мёртвым URL, что и выглядело как «видео просто висит минуту».
          const stalePlaylist =
            data.type === Hls.ErrorTypes.NETWORK_ERROR && data.response?.code === 404;

          const retry = () => {
            if (!hls) return;
            if (stalePlaylist) {
              const position = absoluteTime();
              // Страховка на случай, если пересоздание тоже не выйдет: с сервера позиция
              // читается при следующем открытии, а автосохранение отстаёт до 10 секунд.
              saveProgress(position);
              console.warn(`hls.js: устаревший плейлист, перечитываю с позиции ${position}`);
              hls.destroy();
              hls = null;
              createHls(position);
              return;
            }
            switch (data.type) {
              case Hls.ErrorTypes.NETWORK_ERROR:
                hls.startLoad();
                break;
              case Hls.ErrorTypes.MEDIA_ERROR:
                hls.recoverMediaError();
                break;
              default:
                hls.destroy();
                hls = null;
            }
          };
          setTimeout(retry, 500 * fatalCount);
        });
        // Диагностика рассинхрона «ползунок vs. картинка»: видно, с какой позиции стартовали,
        // какой фрагмент реально подставился в буфер и куда при этом смотрит playhead.
        hls.on(Hls.Events.FRAG_CHANGED, (_e, data) => {
          console.log(
            `hls.js: фрагмент ${data.frag.sn} [${data.frag.start.toFixed(1)}..${(
              data.frag.start + data.frag.duration
            ).toFixed(1)}], currentTime=${video.currentTime.toFixed(1)}`
          );
        });
        hls.loadSource(source);
        hls.attachMedia(video);
        hls.on(Hls.Events.MANIFEST_PARSED, () => {
          console.log(`hls.js: манифест разобран, startPosition=${position}`);
          playIfNeeded();
        });
      };

      // untrack: иначе эффект зависит от startTime и на каждом обновлении позиции
      // пересоздаёт Hls — плеер начинал бы поток заново.
      createHls(untrack(() => resumePosition ?? startTime));
      resumePosition = null;
    } else {
      console.error('HLS не поддерживается этим браузером');
    }

    return () => {
      video.removeEventListener('loadedmetadata', playIfNeeded);
      if (hls) {
        hls.destroy();
        hls = null;
      }
    };
  });

  const selectAudioTrack = (index: number) => {
    if (index === audioIndex) return;
    // Позицию сохраняем и на сервер: пересоздание плеера — единственный момент, когда
    // её легко потерять, а автосохранение отстаёт до 10 секунд.
    const position = absoluteTime();
    resumePosition = position;
    saveProgress(position);
    audioIndex = index;
    try {
      localStorage.setItem(audioTrackKey(mediaId), String(index));
    } catch {
      // Приватный режим/выключенное хранилище — выбор просто не переживёт перезагрузку.
    }
    audioMenuOpen = false;
  };

  // Последняя известная позиция: при размонтировании hls.destroy()/detachMedia
  // успевает сбросить videoEl.currentTime в 0 раньше, чем отработает flushProgress
  let lastKnownTime = untrack(() => startTime);

  // Абсолютная секунда файла: для транскодированного потока время внутри сессии смещено
  const absoluteTime = () => {
    if (!videoEl) return lastKnownTime;
    const t = offsetActive ? streamOffset + videoEl.currentTime : videoEl.currentTime;
    return t > 0 ? t : lastKnownTime;
  };

  let completedSent = false;
  // Отметка «досмотрено» держится только в пределах текущего просмотра: если
  // открыли уже просмотренное заново, первое же сохранение позиции её снимает,
  // иначе повторный просмотр не резюмится и в списке всегда показывает 100%
  let everCompleted = false;

  // Функция сохранения прогресса
  const saveProgress = async (time: number, completed = false) => {
    lastSaveTime = Date.now();
    if (completed) everCompleted = true;
    try {
      await apiClient.updateProgress(currentMediaId, {
        position: Math.floor(time),
        completed: completed || everCompleted,
      });
    } catch (error) {
      console.error('Ошибка сохранения прогресса:', error);
    }
  };

  // Сохранение при уходе со страницы/сворачивании: обычный fetch может не успеть
  // отправиться до выгрузки вкладки, поэтому используем keepalive-запрос — в отличие
  // от navigator.sendBeacon, он позволяет передать Authorization-заголовок,
  // без которого /progress отвечает 401 (в отличие от публичных stream-эндпоинтов).
  const flushProgress = (completed = false) => {
    const time = absoluteTime();
    if (time <= 0) return;
    if (completed) everCompleted = true;
    lastSaveTime = Date.now();
    const token = apiClient.getToken();
    fetch(`${apiClient.getServerUrl()}/api/media/${currentMediaId}/progress`, {
      method: 'PUT',
      keepalive: true,
      headers: {
        'Content-Type': 'application/json',
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
      body: JSON.stringify({ position: Math.floor(time), completed: completed || everCompleted }),
    }).catch(() => {});
  };

  // Обрабатываем изменения src - перезагружаем видео при изменении
  $effect(() => {
    const video = videoEl;
    const source = src;
    if (!video || !source || isHls) return;

    console.log('VideoPlayer: загрузка видео с URL:', source, 'streamOffset:', streamOffset);
    completedSent = false;
    const shouldAutoPlay = transcoded && isPlaying;

    // Обработчик для автовоспроизведения после загрузки.
    // Используем canplaythrough - когда буфер достаточен для воспроизведения
    const handleCanPlayThrough = () => {
      video.play().catch((e) => {
        console.log('Воспроизведение заблокировано:', e.name);
      });
      video.removeEventListener('canplaythrough', handleCanPlayThrough);
    };

    if (shouldAutoPlay) {
      video.addEventListener('canplaythrough', handleCanPlayThrough);
    }

    // Перезагружаем видео при изменении src
    video.load();

    return () => {
      video.removeEventListener('canplaythrough', handleCanPlayThrough);
    };
  });

  // Загружаем сохранённую позицию только для прямого потока: у HLS её ставит startPosition,
  // а у потока с офсетом — сервер.
  $effect(() => {
    const start = startTime;
    if (offsetActive || isHls) return;
    if (videoEl && start > 0 && videoEl.readyState >= 2) {
      videoEl.currentTime = start;
    }
  });

  // Очистка при размонтировании
  $effect(() => {
    return () => {
      if (controlsTimeout) clearTimeout(controlsTimeout);
      flushProgress();
      const video = videoEl;
      if (!video) return;
      video.pause();
      // Очищаем src отложенно — только если элемент действительно ушёл из DOM
      const cleanup = () => {
        if (!document.contains(video)) {
          video.src = '';
          video.load();
        }
      };
      if ('requestIdleCallback' in window) {
        (window as any).requestIdleCallback(cleanup, { timeout: 1000 });
      } else {
        setTimeout(cleanup, 1000);
      }
    };
  });

  // Сохранение при сворачивании/уходе со страницы; при возврате видимости не
  // возобновляем воспроизведение автоматически — пользователь делает это сам
  $effect(() => {
    const handleVisibility = () => {
      if (document.hidden) {
        videoEl?.pause();
        flushProgress();
      }
    };
    const handlePagehide = () => flushProgress();

    document.addEventListener('visibilitychange', handleVisibility);
    window.addEventListener('pagehide', handlePagehide);
    return () => {
      document.removeEventListener('visibilitychange', handleVisibility);
      window.removeEventListener('pagehide', handlePagehide);
    };
  });

  // Автосохранение прогресса каждые 10 секунд
  $effect(() => {
    const playing = isPlaying;
    const interval = setInterval(() => {
      if (videoEl && playing) {
        // Сохраняем только если прошло больше 10 секунд с последнего сохранения
        if (Date.now() - lastSaveTime > 10000) {
          saveProgress(absoluteTime());
        }
      }
    }, 10000);

    return () => clearInterval(interval);
  });

  // Отмечаем как просмотрено один раз при достижении 90%
  $effect(() => {
    if (!completedSent && duration && currentTime > 0 && (currentTime / duration) * 100 >= 90) {
      completedSent = true;
      saveProgress(currentTime, true);
    }
  });

  // Длительность из метаданных точнее записанной в БД; на неё же ориентируется автопереход
  let mediaDuration = $state(0);
  let effectiveDuration = $derived(mediaDuration > 0 ? mediaDuration : duration || 0);

  let remaining = $derived(effectiveDuration ? effectiveDuration - currentTime : Infinity);

  // Оверлей «Следующий эпизод»: за NEXT_EP_THRESHOLD секунд до конца при активном воспроизведении
  let showNextOverlay = $derived(
    !!onNextEpisode &&
      !!effectiveDuration &&
      isPlaying &&
      remaining > 0 &&
      remaining <= NEXT_EP_THRESHOLD
  );

  // Автопереход к следующему эпизоду. На событие `ended` полагаться нельзя:
  // при HLS оно может не прийти (последний сегмент не догрузился/поток встал),
  // поэтому дополнительно срабатываем по достижении конца по времени.
  let nextTriggered = false;
  const advanceToNext = () => {
    if (nextTriggered || !onNextEpisode) return;
    nextTriggered = true;
    flushProgress(true);
    onNextEpisode();
  };

  // Стартовая позиция может сама попадать в хвост файла (например, при ручной
  // перемотке в конец) — переключаем только если с неё действительно проигралось время
  let initialTime = untrack(() => startTime);

  // Кнопка play превращается в «заново»: у досмотренного до старта и в самом конце файла
  let atEnd = $derived(effectiveDuration > 0 && remaining <= 0.5);
  let showRestart = $derived(!isPlaying && (atEnd || (watched && currentTime < 1)));

  $effect(() => {
    if (!onNextEpisode || !effectiveDuration || currentTime <= 0) return;
    if (currentTime <= initialTime + 1) return;
    if (remaining <= 0.5) advanceToNext();
  });

  const handlePlayPause = async () => {
    if (!videoEl) return;
    // Досмотренное запускаем с начала: кнопка в этом состоянии — «заново»
    if (!isPlaying && showRestart) {
      nextTriggered = false;
      initialTime = 0;
      seekTo(0);
    }
    try {
      if (isPlaying) {
        videoEl.pause();
        isPlaying = false;
        saveProgress(absoluteTime());
      } else {
        const playPromise = videoEl.play();
        if (playPromise !== undefined) {
          await playPromise;
        }
        isPlaying = true;
      }
    } catch (error) {
      console.error('Ошибка воспроизведения:', error);
      isPlaying = false;
    }
  };

  /// Перемотка на абсолютную секунду файла.
  const seekTo = (time: number) => {
    if (!videoEl) return;
    const target = Math.max(0, duration ? Math.min(time, duration) : time);

    if (isHls) {
      // Плейлист покрывает весь файл — перемотка это просто смена currentTime,
      // сегменты для новой позиции сервер сгенерирует лениво по запросу.
      videoEl.currentTime = target;
      currentTime = target;
      saveProgress(target);
      return;
    }

    videoEl.currentTime = target;
    currentTime = target;
    saveProgress(target);
  };

  const skip = (delta: number) => {
    resetControlsTimeout();
    seekTo(currentTime + delta);
  };

  const handleSeek = (e: Event) => {
    const time = parseFloat((e.currentTarget as HTMLInputElement).value);
    if (!videoEl) return;
    if (isHls) {
      seekTo(time);
      return;
    }
    try {
      // Для транскодированного потока обновляем URL с параметром start.
      // Обновляем только если перемотка больше 10 секунд от текущей позиции
      if (transcoded && onSrcChange && Math.abs(time - currentTime) > 10) {
        const newSrc = apiClient.getStreamUrl(mediaId, true, time);
        // Обновляем смещение потока
        streamOffset = time;
        currentTime = time; // Обновляем UI сразу
        saveProgress(time);
        onSrcChange(newSrc);
        // НЕ устанавливаем currentTime здесь - видео начнёт с 0 (т.к. сервер транскодирует с time)
        return;
      }

      // Устанавливаем время напрямую для не-транскодированного потока
      videoEl.currentTime = time;
      currentTime = time;
      saveProgress(time);
      // Если видео еще не загружено, ждем загрузки метаданных
      if (videoEl.readyState < 2) {
        const video = videoEl;
        const handleCanSeek = () => {
          video.currentTime = time;
          video.removeEventListener('loadedmetadata', handleCanSeek);
        };
        video.addEventListener('loadedmetadata', handleCanSeek);
      }
    } catch (error) {
      console.error('Ошибка перемотки:', error);
    }
  };

  // Фуллскрин берёт контейнер, а не <video>: у видео в полноэкранном режиме
  // остаются только нативные контролы, наша панель и оверлей пропадают
  const handleFullscreen = () => {
    if (!playerEl) return;
    if (document.fullscreenElement) {
      document.exitFullscreen().catch(() => {});
    } else {
      playerEl.requestFullscreen().catch(() => {});
    }
  };

  $effect(() => {
    const sync = () => (isFullscreen = document.fullscreenElement === playerEl);
    document.addEventListener('fullscreenchange', sync);
    return () => document.removeEventListener('fullscreenchange', sync);
  });

  const handleTimeUpdate = () => {
    if (!videoEl) return;
    const videoTime = videoEl.currentTime;
    // Поток с офсетом: сервер начинает с streamOffset,
    // поэтому реальное время = streamOffset + videoTime
    const displayTime = offsetActive ? streamOffset + videoTime : videoTime;

    currentTime = displayTime;
    if (displayTime > 0) lastKnownTime = displayTime;
    onTimeUpdate?.(displayTime);
  };

  const formatTime = (seconds: number) => {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    if (h > 0) {
      return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
    }
    return `${m}:${s.toString().padStart(2, '0')}`;
  };

  // Фокус внутри интерфейса плеера (контролы/оверлей), а не «припаркован»
  const focusInUi = (): boolean => {
    const active = document.activeElement;
    return !!active && !!playerEl && playerEl.contains(active) && active !== idleEl;
  };

  const parkFocus = () => idleEl?.focus({ preventScroll: true });

  // Показать контролы и отдать фокус полосе прогресса — точка входа в панель с пульта
  const revealControls = (target?: HTMLElement | null) => {
    resetControlsTimeout();
    (target ?? seekEl)?.focus({ preventScroll: true });
  };

  // Скрытие контролов при бездействии
  const resetControlsTimeout = () => {
    if (controlsTimeout) clearTimeout(controlsTimeout);
    showControls = true;
    controlsTimeout = window.setTimeout(() => {
      if (!isPlaying) return;
      showControls = false;
      // Панель уехала — фокус нельзя оставлять на невидимой кнопке
      if (focusInUi()) parkFocus();
    }, 3000);
  };

  // Стартовое состояние: фокус у плеера, иначе первая же стрелка уводит его
  // на кнопку в скрытой панели
  $effect(() => {
    if (idleEl) parkFocus();
  });

  $effect(() => {
    // Перезапускаем таймер при смене состояния воспроизведения
    isPlaying;
    resetControlsTimeout();
  });

  // Первый фокусируемый элемент над полосой прогресса: оверлей «следующий эпизод»,
  // иначе кнопки поверх видео («Назад», заголовок)
  const focusAbove = () => {
    const el =
      playerEl?.querySelector<HTMLElement>('.next-ep button') ??
      playerEl?.querySelector<HTMLElement>('.player__overlay button, .player__overlay a');
    if (el) el.focus({ preventScroll: true });
  };

  // Клавиши на полосе прогресса: влево/вправо перематывают (а не двигают ползунок
  // нативно), вверх/вниз выводят из неё — spatial-навигация для range отключена
  const handleSeekKeyDown = (e: KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowLeft':
        e.preventDefault();
        skip(-10);
        break;
      case 'ArrowRight':
        e.preventDefault();
        skip(10);
        break;
      case 'ArrowUp':
        e.preventDefault();
        focusAbove();
        break;
      case 'ArrowDown':
        e.preventDefault();
        playEl?.focus({ preventScroll: true });
        break;
      case ' ':
      case 'Enter':
        e.preventDefault();
        handlePlayPause();
        break;
    }
  };

  // Горячие клавиши. Слушаем в фазе перехвата, чтобы опередить глобальную
  // spatial-навигацию (её слушатель на window тоже) и забрать у неё Back.
  $effect(() => {
    const overlayActive = showNextOverlay;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (!videoEl) return;

      // Back/Escape при открытой панели — сначала выход из неё, а не из плеера
      if (e.key === 'Escape' || e.keyCode === 461 || e.keyCode === 10009) {
        if (!focusInUi()) return;
        e.preventDefault();
        e.stopPropagation();
        parkFocus();
        return;
      }

      // Фокус в панели — клавишами распоряжаются сами контролы и spatial-навигация,
      // наше дело только не дать панели спрятаться под руками
      if (focusInUi()) {
        resetControlsTimeout();
        if (e.key === 'f' || e.key === 'F') {
          e.preventDefault();
          handleFullscreen();
        }
        return;
      }

      switch (e.key) {
        case ' ':
        case 'Enter': // OK на пульте
          e.preventDefault();
          // Если показан оверлей «Следующий эпизод» — OK переходит к нему
          if (overlayActive && onNextEpisode) {
            advanceToNext();
            break;
          }
          handlePlayPause();
          revealControls();
          break;
        // Перемотка вслепую: панель намеренно не показываем
        case 'ArrowLeft':
          e.preventDefault();
          seekTo(currentTime - 10);
          break;
        case 'ArrowRight':
          e.preventDefault();
          seekTo(currentTime + 10);
          break;
        case 'ArrowUp':
        case 'ArrowDown':
          e.preventDefault();
          revealControls();
          break;
        case 'f':
        case 'F':
          e.preventDefault();
          handleFullscreen();
          break;
      }
    };

    window.addEventListener('keydown', handleKeyDown, true);
    return () => window.removeEventListener('keydown', handleKeyDown, true);
  });

  // Обработчик клика для включения звука (если браузер заблокировал автовоспроизведение)
  const handleVideoClick = () => {
    if (!videoEl) return;
    if (videoEl.muted) videoEl.muted = false;
    handlePlayPause();
  };

  const handleError = (e: Event) => {
    const video = e.currentTarget as HTMLVideoElement;
    const error = video.error;
    if (!error) return;
    let errorMsg = 'Неизвестная ошибка';
    switch (error.code) {
      case error.MEDIA_ERR_ABORTED:
        errorMsg = 'Воспроизведение прервано';
        break;
      case error.MEDIA_ERR_NETWORK:
        errorMsg = 'Ошибка сети при загрузке видео';
        break;
      case error.MEDIA_ERR_DECODE:
        errorMsg = 'Ошибка декодирования видео';
        break;
      case error.MEDIA_ERR_SRC_NOT_SUPPORTED:
        errorMsg = 'Формат видео не поддерживается или источник недоступен';
        break;
    }
    console.error('Ошибка видео:', {
      code: error.code,
      message: errorMsg,
      src: video.src,
      networkState: video.networkState,
      readyState: video.readyState,
    });
  };

  const handleLoadedMetadata = () => {
    if (!videoEl) return;
    // Проверяем наличие аудио дорожек
    try {
      const audioTracks = (videoEl as any).audioTracks;
      if (audioTracks && audioTracks.length > 0) {
        console.log('Аудио дорожки найдены:', audioTracks.length);
        for (let i = 0; i < audioTracks.length; i++) {
          audioTracks[i].enabled = true; // Включаем все аудио дорожки
        }
      } else {
        console.log('audioTracks API недоступно или дорожек нет');
      }
    } catch (e) {
      console.log('Не удалось проверить audioTracks:', e);
    }

    if (audioCodec) {
      // Проверяем поддержку кодека браузером
      const supportedCodecs = ['aac', 'mp3', 'opus', 'vorbis', 'flac'];
      const codecLower = audioCodec.toLowerCase();
      const isSupported = supportedCodecs.some((codec) => codecLower.includes(codec));

      if (!isSupported) {
        console.warn(`⚠️ Кодек "${audioCodec}" может не поддерживаться браузером нативно.`);
        console.warn('Поддерживаемые кодеки: AAC, MP3, Opus, Vorbis, FLAC');
        console.warn('Для AC3/DTS требуется транскодирование на сервере.');
      }
    } else {
      console.warn(
        '⚠️ Аудио кодек не указан в метаданных - возможно, видео без звука или кодек не определен'
      );
    }

    // Позицию HLS задаёт только startPosition в конфиге hls.js. Ручной seek здесь ставил её
    // второй раз: плеер успевал начать с нуля, потом прыгал на сохранённую секунду, hls.js
    // перезапрашивал init и уходил тянуть сегменты до конца файла, ни разу не начав играть.
  };
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  bind:this={playerEl}
  class="player"
  onmousemove={resetControlsTimeout}
  onmouseleave={() => {
    if (isPlaying) showControls = false;
  }}
  onclick={handleVideoClick}
>
  <!-- svelte-ignore a11y_media_has_caption -->
  <video
    bind:this={videoEl}
    src={isHls ? undefined : src}
    class="player__video"
    preload="metadata"
    playsinline
    muted={false}
    crossorigin="anonymous"
    autoplay={autoplay}
    onerror={handleError}
    ontimeupdate={handleTimeUpdate}
    onended={() => {
      isPlaying = false;
      if (onNextEpisode) {
        advanceToNext();
      } else {
        onEnded?.();
      }
    }}
    ondurationchange={() => {
      const d = videoEl?.duration;
      if (d && Number.isFinite(d)) mediaDuration = offsetActive ? streamOffset + d : d;
    }}
    onplay={() => (isPlaying = true)}
    onpause={() => {
      isPlaying = false;
      flushProgress();
    }}
    onloadedmetadata={handleLoadedMetadata}
  ></video>

  <!-- Держатель фокуса на время просмотра: не участвует в spatial-навигации -->
  <div bind:this={idleEl} class="player__idle" tabindex="-1" data-no-spatial aria-hidden="true"></div>

  {#if playbackError}
    <div class="player__error">{playbackError}</div>
  {/if}

  {#if overlay}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="player__overlay"
      class:player__overlay--hidden={!showControls}
      onclick={(e) => e.stopPropagation()}
    >
      {@render overlay()}
    </div>
  {/if}

  <!-- Оверлей «Следующий эпизод» с обратным отсчётом -->
  {#if showNextOverlay && onNextEpisode}
    <div class="next-ep">
      <button
        onclick={(e) => {
          e.stopPropagation();
          advanceToNext();
        }}
        class="next-ep__btn"
      >
        <span class="next-ep__countdown">Следующий эпизод через {Math.ceil(remaining)}…</span>
        <span class="next-ep__label">▶ Смотреть далее</span>
      </button>
    </div>
  {/if}

  <!-- Контролы -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="controls"
    class:controls--hidden={!showControls}
    onclick={(e) => e.stopPropagation()}
  >
    <!-- Прогресс-бар -->
    <div class="controls__seek">
      <input
        bind:this={seekEl}
        type="range"
        min="0"
        max={duration || 0}
        value={currentTime}
        oninput={handleSeek}
        onkeydown={handleSeekKeyDown}
        data-no-spatial
        aria-label="Перемотка"
        class="range range--seek"
        style="--played: {(currentTime / (duration || 1)) * 100}%"
      />
    </div>

    <!-- Кнопки управления -->
    <div class="controls__row">
      <div class="controls__group">
        <button onclick={() => skip(-10)} class="btn--icon" title="Назад 10 сек">
          <svg class="icon icon--lg" fill="currentColor" viewBox="0 -960 960 960">
            <path d="M360-522h-30q-10.4 0-17.2-7.12-6.8-7.11-6.8-18 0-10.88 7.08-17.38 7.09-6.5 17.92-6.5h55q11 0 17.5 6.5T410-547v212q0 10.83-7.12 17.92-7.11 7.08-18 7.08-10.88 0-17.88-7.08-7-7.09-7-17.92v-187Zm147 212q-18.7 0-31.35-12.65Q463-335.3 463-354v-173q0-18.7 12.65-31.35Q488.3-571 507-571h83q18.7 0 31.35 12.65Q634-545.7 634-527v173q0 18.7-12.65 31.35Q608.7-310 590-310h-83Zm6-50h71v-162h-71v162ZM339.5-108Q274-136 225-185t-77-114.5Q120-365 120-440q0-12.75 8.68-21.38 8.67-8.62 21.5-8.62 12.82 0 21.32 8.62 8.5 8.63 8.5 21.38 0 125.36 87.5 212.68Q355-140 480-140t212.5-87.32Q780-314.64 780-440q0-125.36-85-212.68Q610-740 485-740h-22l52 52q9 9 9 21t-8.61 21q-9.39 9-21.39 9t-21-9L368-751q-9-9-9-21t9-21l106-106q8-8 20.5-8t20.85 8q7.65 8 7.65 20.5t-8 20.5l-58 58h23q75 0 140.5 28T735-695q49 49 77 114.5T840-440q0 75-28 140.5T735-185q-49 49-114.5 77T480-80q-75 0-140.5-28Z" />
          </svg>
        </button>

        <button
          bind:this={playEl}
          onclick={handlePlayPause}
          class="btn--icon"
          title={isPlaying ? 'Пауза' : showRestart ? 'Смотреть заново' : 'Играть'}
        >
          {#if isPlaying}
            <svg class="icon icon--lg" fill="currentColor" viewBox="0 -960 960 960">
            <path d="M615-200q-24.75 0-42.37-17.63Q555-235.25 555-260v-440q0-24.75 17.63-42.38Q590.25-760 615-760h55q24.75 0 42.38 17.62Q730-724.75 730-700v440q0 24.75-17.62 42.37Q694.75-200 670-200h-55Zm-325 0q-24.75 0-42.37-17.63Q230-235.25 230-260v-440q0-24.75 17.63-42.38Q265.25-760 290-760h55q24.75 0 42.38 17.62Q405-724.75 405-700v440q0 24.75-17.62 42.37Q369.75-200 345-200h-55Z" />
          </svg>
          {:else if showRestart}
            <!-- Круговая стрелка: запуск досмотренного заново -->
            <svg class="icon icon--lg" fill="currentColor" viewBox="0 -960 960 960">
            <path d="M339.5-108Q274-136 225-185t-77-114.5Q120-365 120-440q0-13 8.5-21.5T150-470q13 0 21.5 8.5T180-440q0 125 87.5 212.5T480-140q125 0 212.5-87.5T780-440q0-125-85-212.5T485-740h-22l52 52q9 9 9 21t-9 21q-9 9-21 9t-21-9L368-751q-9-9-9-21t9-21l106-106q8-8 20.5-8t20.5 8q8 8 8 20.5t-8 20.5l-58 58h23q75 0 140.5 28T735-695q49 49 77 114.5T840-440q0 75-28 140.5T735-185q-49 49-114.5 77T480-80q-75 0-140.5-28Z" />
          </svg>
          {:else}
            <svg class="icon icon--lg" fill="currentColor" viewBox="0 -960 960 960">
            <path d="M320-258v-450q0-14 9-22t21-8q4 0 8 1t8 3l354 226q7 5 10.5 11t3.5 14q0 8-3.5 14T720-458L366-232q-4 2-8 3t-8 1q-12 0-21-8t-9-22Z" />
          </svg>
          {/if}
        </button>

        <button onclick={() => skip(10)} class="btn--icon" title="Вперёд 10 сек">
          <svg class="icon icon--lg" fill="currentColor" viewBox="0 -960 960 960">
            <path d="M339.5-108Q274-136 225-185t-77-114.5Q120-365 120-440t28-140.5Q176-646 225-695t114.5-77Q405-800 480-800h23l-57-57q-8-8-8-20.5t7.65-20.5q8.35-8 20.35-8.5 12-.5 20 7.5l106 106q9 9 9 21t-9 21L487-646q-9 9-21 9t-21.39-9q-8.61-9-8.61-21t9-21l52-52h-22q-125 0-210 87.32T180-440q0 125.36 87.5 212.68Q355-140 480-140t212.5-87.32Q780-314.64 780-440q0-12.75 8.68-21.38 8.67-8.62 21.5-8.62 12.82 0 21.32 8.62 8.5 8.63 8.5 21.38 0 75-28 140.5T735-185q-49 49-114.5 77T480-80q-75 0-140.5-28ZM360-522h-30q-10.4 0-17.2-7.12-6.8-7.11-6.8-18 0-10.88 7.08-17.38 7.09-6.5 17.92-6.5h55q11 0 17.5 6.5T410-547v212q0 10.83-7.12 17.92-7.11 7.08-18 7.08-10.88 0-17.88-7.08-7-7.09-7-17.92v-187Zm147 212q-18.7 0-31.35-12.65Q463-335.3 463-354v-173q0-18.7 12.65-31.35Q488.3-571 507-571h83q18.7 0 31.35 12.65Q634-545.7 634-527v173q0 18.7-12.65 31.35Q608.7-310 590-310h-83Zm6-50h71v-162h-71v162Z" />
          </svg>
        </button>

        <div class="controls__time">
          {formatTime(currentTime)} / {formatTime(duration || 0)}
        </div>
      </div>

      <div class="controls__group">
        {#if audioTracks.length > 1}
          <div class="audio-menu">
            {#if audioMenuOpen}
              <ul class="audio-menu__list">
                {#each audioTracks as track (track.index)}
                  <li>
                    <button
                      class="audio-menu__item"
                      class:audio-menu__item--active={track.index === audioIndex}
                      onclick={() => selectAudioTrack(track.index)}
                    >
                      {audioTrackLabel(track)}
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
            <button
              onclick={() => (audioMenuOpen = !audioMenuOpen)}
              class="btn--icon"
              title="Аудиодорожка"
              aria-expanded={audioMenuOpen}
            >
              <svg class="icon icon--md" fill="currentColor" viewBox="0 -960 960 960">
                <path d="M280-360q-17 0-28.5-11.5T240-400v-160q0-17 11.5-28.5T280-600h100l114-114q19-19 42.5-8.5T560-686v412q0 26-23.5 36.5T494-246L380-360H280Zm400-120q0 32-14 60t-38 45q-9 5-18.5 0t-9.5-16v-179q0-11 9.5-16t18.5 0q24 18 38 46t14 60Z" />
              </svg>
            </button>
          </div>
        {/if}

        <button
          onclick={handleFullscreen}
          class="btn--icon"
          title={isFullscreen ? 'Выйти из полного экрана' : 'Полный экран'}
        >
          <svg class="icon icon--md" fill="currentColor" viewBox="0 -960 960 960">
            <path d="M180-180h103q12.75 0 21.38 8.68 8.62 8.67 8.62 21.5 0 12.82-8.62 21.32-8.63 8.5-21.38 8.5H150q-12.75 0-21.37-8.63Q120-137.25 120-150v-133q0-12.75 8.68-21.38 8.67-8.62 21.5-8.62 12.82 0 21.32 8.62 8.5 8.63 8.5 21.38v103Zm600 0v-103q0-12.75 8.68-21.38 8.67-8.62 21.5-8.62 12.82 0 21.32 8.62 8.5 8.63 8.5 21.38v133q0 12.75-8.62 21.37Q822.75-120 810-120H677q-12.75 0-21.37-8.68-8.63-8.67-8.63-21.5 0-12.82 8.63-21.32 8.62-8.5 21.37-8.5h103ZM180-780v103q0 12.75-8.68 21.37-8.67 8.63-21.5 8.63-12.82 0-21.32-8.63-8.5-8.62-8.5-21.37v-133q0-12.75 8.63-21.38Q137.25-840 150-840h133q12.75 0 21.38 8.68 8.62 8.67 8.62 21.5 0 12.82-8.62 21.32-8.63 8.5-21.38 8.5H180Zm600 0H677q-12.75 0-21.37-8.68-8.63-8.67-8.63-21.5 0-12.82 8.63-21.32 8.62-8.5 21.37-8.5h133q12.75 0 21.38 8.62Q840-822.75 840-810v133q0 12.75-8.68 21.37-8.67 8.63-21.5 8.63-12.82 0-21.32-8.63-8.5-8.62-8.5-21.37v-103Z" />
          </svg>
        </button>
      </div>
    </div>
  </div>
</div>

<style>
  .player {
    position: relative;
    width: 100%;
    height: 100%;
    background-color: var(--bg-cinema);
  }

  .player__video {
    width: 100%;
    height: 100%;
  }

  /* Нулевой размер: элемент нужен только чтобы держать фокус, не показываться */
  .player__idle {
    position: absolute;
    top: 0;
    left: 0;
    width: 0;
    height: 0;
    outline: none;
  }

  .player__overlay {
    position: absolute;
    top: var(--space-4);
    left: var(--space-4);
    transition:
      opacity var(--transition),
      visibility var(--transition);
  }

  .player__overlay--hidden {
    opacity: 0;
    visibility: hidden;
    pointer-events: none;
  }

  .player__error {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    padding: var(--space-4) var(--space-5);
    border-radius: var(--radius);
    background-color: var(--overlay);
    color: var(--text);
    text-align: center;
  }

  /* ── Оверлей «Следующий эпизод» ─────────────────────────────── */

  .next-ep {
    position: absolute;
    right: var(--space-6);
    bottom: 7rem;
    z-index: var(--z-raised);
  }

  .next-ep__btn {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-1);
    padding: var(--space-4) var(--space-5);
    border: none;
    border-radius: var(--radius);
    background-color: var(--overlay);
    color: var(--text);
    text-align: left;
    box-shadow: var(--shadow-lg);
    cursor: pointer;
    transition: background-color var(--transition);
  }

  .next-ep__btn:hover {
    background-color: var(--overlay-strong);
  }

  .next-ep__countdown {
    font-size: var(--text-sm);
    color: var(--text-on-media);
  }

  .next-ep__label {
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
  }

  /* ── Панель управления ───────────────────────────────────────── */

  .controls {
    position: absolute;
    inset-inline: 0;
    bottom: 0;
    padding: var(--space-4);
    background-image: linear-gradient(to top, var(--overlay), transparent);
    opacity: 1;
    transition:
      opacity var(--transition),
      visibility var(--transition);
  }

  /* visibility, а не только opacity: иначе фокус уходит на невидимые кнопки */
  .controls--hidden {
    opacity: 0;
    visibility: hidden;
    pointer-events: none;
  }

  .controls__seek {
    margin-bottom: var(--space-4);
  }

  .controls__row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .controls__group {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .controls__time {
    font-size: var(--text-sm);
  }

  /* ── Выбор аудиодорожки ──────────────────────────────────────── */

  .audio-menu {
    position: relative;
  }

  .audio-menu__list {
    position: absolute;
    right: 0;
    bottom: calc(100% + var(--space-2));
    z-index: var(--z-raised);
    min-width: 16rem;
    max-height: 40vh;
    overflow-y: auto;
    padding: var(--space-2);
    border-radius: var(--radius);
    background-color: var(--surface-2);
    list-style: none;
  }

  .audio-menu__item {
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: 0;
    border-radius: var(--radius);
    background: none;
    color: inherit;
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }

  .audio-menu__item:hover,
  .audio-menu__item:focus-visible {
    background-color: var(--surface-3);
  }

  .audio-menu__item--active {
    color: var(--accent);
  }

  .icon {
    width: 1.25rem;
    height: 1.25rem;
  }

  .icon--md {
    width: 1.5rem;
    height: 1.5rem;
  }

  .icon--lg {
    width: 2rem;
    height: 2rem;
  }

  /* ── Ползунки: перемотка и громкость ─────────────────────────── */

  .range {
    appearance: none;
    -webkit-appearance: none;
    background-color: var(--surface-2);
    border-radius: var(--radius);
    cursor: pointer;
  }

  .range--seek {
    width: 100%;
    height: 8px;
    /* --played задаётся инлайново из currentTime */
    background-image: linear-gradient(
      to right,
      var(--accent) 0%,
      var(--accent) var(--played, 0%),
      var(--surface-2) var(--played, 0%),
      var(--surface-2) 100%
    );
  }

  .range::-webkit-slider-thumb {
    appearance: none;
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    border-radius: var(--radius-full);
    background-color: var(--text);
  }

  .range::-moz-range-thumb {
    width: 14px;
    height: 14px;
    border: none;
    border-radius: var(--radius-full);
    background-color: var(--text);
  }
</style>
