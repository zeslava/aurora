export type MediaType = 'movie' | 'tvseries';

/// Технические характеристики файла — общая часть фильма и эпизода
export interface MediaFileInfo {
  file_path: string;
  duration?: number;
  resolution?: string;
  video_codec?: string;
  audio_codec?: string;
  video_bitrate?: number;
  pix_fmt?: string;
}

/// Метаданные TMDB. poster/backdrop — относительные пути на этот же сервер
export interface TmdbInfo {
  tmdb_id?: number;
  overview?: string;
  rating?: number;
  poster?: string | null;
  backdrop?: string | null;
}

export interface MediaItem extends MediaFileInfo, TmdbInfo {
  id: string;
  library_id?: number;
  title: string;
  custom_title?: string;
  year?: number;
  type: MediaType;
  created_at?: string;
}

export interface Library {
  id: number;
  name: string;
  path: string;
  type: MediaType;
}

export interface Episode extends MediaFileInfo {
  id: string;
  series_id: string;
  season_number: number;
  episode_number: number;
  title?: string;
  created_at?: string;
}

export interface PlaybackState {
  user_id: number;
  media_id: string;
  position: number;
  completed: boolean;
  last_watched?: string;
}

export interface AuthResponse {
  token: string;
  user_id: number;
  username: string;
}

export interface LoginRequest {
  username: string;
  password: string;
}

export interface RegisterRequest {
  username: string;
  password: string;
}

export interface ProgressUpdateRequest {
  position: number;
  completed?: boolean;
}

export interface ContinueWatchingItem {
  id: string;
  title: string;
  type: MediaType;
  position: number;
  duration?: number;
  last_watched?: string;
  poster?: string | null;
  backdrop?: string | null;
  series_id?: string;
  season_number?: number;
  episode_number?: number;
  episode_title?: string;
}

export interface HomeResponse {
  continue_watching: ContinueWatchingItem[];
  recently_added: MediaItem[];
}

export interface SeriesEpisodesResponse {
  series_id: string;
  seasons: Record<string, Episode[]>;
  /** URL обложки по номеру сезона; сезона может не быть в словаре */
  season_posters: Record<string, string>;
}

