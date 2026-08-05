import axios from 'axios';
import type { AxiosInstance, AxiosError } from 'axios';
import type {
  AuthResponse,
  LoginRequest,
  RegisterRequest,
  MediaItem,
  Library,
  PlaybackState,
  ProgressUpdateRequest,
  SeriesEpisodesResponse,
  HomeResponse,
} from '../types';

// Адрес сервера, зашитый на этапе сборки (пусто = относительный URL / тот же origin)
const BUILD_API_URL = import.meta.env.VITE_API_URL || '';
const SERVER_URL_KEY = 'server_url';

// Рантайм-адрес сервера имеет приоритет над сборочным: один и тот же .ipk
// (webOS) работает с любым сервером — пользователь вводит адрес в приложении
const getConfiguredServerUrl = (): string =>
  localStorage.getItem(SERVER_URL_KEY) || BUILD_API_URL;

// Нормализация: без хвостового слэша, чтобы не плодить `//api`
const normalizeUrl = (url: string): string => url.trim().replace(/\/+$/, '');

class ApiClient {
  private client: AxiosInstance;
  private token: string | null = null;
  private baseUrl: string = getConfiguredServerUrl();
  // Колбэк роутера: переход на экран входа без перезагрузки страницы
  // (жёсткий window.location сломал бы hash-роутинг в упакованном приложении)
  private unauthorizedHandler: (() => void) | null = null;

  constructor() {
    this.client = axios.create({
      baseURL: this.baseUrl,
      headers: {
        'Content-Type': 'application/json',
      },
    });

    // Загружаем токен из localStorage
    this.token = localStorage.getItem('auth_token');
    if (this.token) {
      this.setToken(this.token);
    }

    // Перехватчик для добавления токена
    this.client.interceptors.request.use((config) => {
      if (this.token) {
        config.headers.Authorization = `Bearer ${this.token}`;
      }
      return config;
    });

    // Перехватчик для обработки ошибок
    this.client.interceptors.response.use(
      (response) => response,
      (error: AxiosError) => {
        if (error.response?.status === 401) {
          // Токен невалиден, очищаем и редиректим на логин
          this.clearToken();
          this.unauthorizedHandler?.();
        }
        return Promise.reject(error);
      }
    );
  }

  onUnauthorized(handler: () => void) {
    this.unauthorizedHandler = handler;
  }

  // Адрес сервера (для webOS-приложения, где нет своего origin)
  getServerUrl(): string {
    return this.baseUrl;
  }

  setServerUrl(url: string) {
    this.baseUrl = normalizeUrl(url);
    localStorage.setItem(SERVER_URL_KEY, this.baseUrl);
    this.client.defaults.baseURL = this.baseUrl;
  }

  clearServerUrl() {
    this.baseUrl = BUILD_API_URL;
    localStorage.removeItem(SERVER_URL_KEY);
    this.client.defaults.baseURL = this.baseUrl;
  }

  // Запущено ли приложение как упакованный .ipk (file://), а не как веб-страница
  isPackagedApp(): boolean {
    return window.location.protocol !== 'http:' && window.location.protocol !== 'https:';
  }

  // Требуется ли ручной ввод адреса: упакованное приложение без настроенного адреса
  needsServerSetup(): boolean {
    return this.isPackagedApp() && !this.baseUrl;
  }

  // Проверка доступности текущего сервера (с таймаутом)
  async pingServer(timeoutMs = 5000): Promise<boolean> {
    if (!this.baseUrl) return false;
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), timeoutMs);
    try {
      await fetch(`${this.baseUrl}/api/libraries`, { signal: ctrl.signal });
      return true;
    } catch {
      return false;
    } finally {
      clearTimeout(t);
    }
  }

  getToken(): string | null {
    return this.token;
  }

  setToken(token: string) {
    this.token = token;
    localStorage.setItem('auth_token', token);
    this.client.defaults.headers.common['Authorization'] = `Bearer ${token}`;
  }

  clearToken() {
    this.token = null;
    localStorage.removeItem('auth_token');
    delete this.client.defaults.headers.common['Authorization'];
  }

  // Аутентификация
  async register(data: RegisterRequest): Promise<AuthResponse> {
    const response = await this.client.post<AuthResponse>('/api/auth/register', data);
    this.setToken(response.data.token);
    return response.data;
  }

  async login(data: LoginRequest): Promise<AuthResponse> {
    const response = await this.client.post<AuthResponse>('/api/auth/login', data);
    this.setToken(response.data.token);
    return response.data;
  }

  // Сопряжение устройств (pairing-код)
  async pairStart(): Promise<{ pairing_id: string; code: string }> {
    const response = await this.client.post<{ pairing_id: string; code: string }>('/api/pair/start');
    return response.data;
  }

  async pairConfirm(code: string): Promise<void> {
    await this.client.post('/api/pair/confirm', { code });
  }

  // Возвращает токен, если код подтверждён; null — пока ожидание; бросает при истечении (404)
  async pairPoll(pairingId: string): Promise<AuthResponse | null> {
    const response = await this.client.post<{ status: string } & Partial<AuthResponse>>(
      '/api/pair/poll',
      { pairing_id: pairingId }
    );
    if (response.data.status === 'confirmed' && response.data.token) {
      const { token, user_id, username } = response.data;
      this.setToken(token);
      return { token, user_id: user_id!, username: username! };
    }
    return null;
  }

  // Библиотеки
  async getLibraries(): Promise<Library[]> {
    const response = await this.client.get<Library[]>('/api/libraries');
    return response.data;
  }

  async scanLibraries(): Promise<{ status: string; count: number }> {
    const response = await this.client.post<{ status: string; count: number }>('/api/libraries/scan');
    return response.data;
  }

  // Главный экран
  async getHome(): Promise<HomeResponse> {
    const response = await this.client.get<HomeResponse>('/api/home');
    return response.data;
  }

  // Медиа
  async getMedia(type?: 'movie' | 'tv'): Promise<MediaItem[]> {
    const params = type ? { type } : {};
    const response = await this.client.get<MediaItem[]>('/api/media', { params });
    return response.data;
  }

  async getMediaById(id: string): Promise<MediaItem> {
    const response = await this.client.get<MediaItem>(`/api/media/${id}`);
    return response.data;
  }

  /** Пустая строка снимает пользовательское название и возвращает распарсенное из файла */
  async setCustomTitle(id: string, customTitle: string): Promise<MediaItem> {
    const response = await this.client.patch<MediaItem>(`/api/media/${id}`, {
      custom_title: customTitle,
    });
    return response.data;
  }

  getStreamUrl(id: string, transcoded: boolean = false, startTime?: number): string {
    // HTML5 video элемент не может отправлять заголовки Authorization,
    // поэтому передаем токен через query параметр
    const token = this.token || localStorage.getItem('auth_token');
    const params = new URLSearchParams();
    if (token) {
      params.set('token', token);
    }
    if (transcoded && startTime !== undefined && startTime > 0) {
      params.set('start', Math.floor(startTime).toString());
    }
    const queryString = params.toString();
    const tokenParam = queryString ? `?${queryString}` : '';
    
    const baseUrl = this.baseUrl;
    const url = transcoded
      ? `${baseUrl}/api/media/${id}/stream-transcoded${tokenParam}`
      : `${baseUrl}/api/media/${id}/stream${tokenParam}`;
    
    console.log('getStreamUrl:', { baseUrl, url, transcoded, hasToken: !!token, startTime });
    return url;
  }

  getHlsUrl(id: string): string {
    return `${this.baseUrl}/api/media/${id}/hls/playlist.m3u8`;
  }

  // Абсолютный URL картинки: бэкенд отдаёт относительный путь, а в упакованном
  // приложении origin чужой — приклеиваем адрес сервера
  getImageUrl(path?: string | null): string | null {
    if (!path) return null;
    return `${this.baseUrl}${path}`;
  }

  // Перезапросить метаданные TMDB для всей библиотеки
  async refreshMetadata(): Promise<void> {
    await this.client.post('/api/metadata/refresh');
  }

  // Прогресс просмотра
  async getProgress(mediaId: string): Promise<PlaybackState | null> {
    try {
      const response = await this.client.get<PlaybackState>(`/api/media/${mediaId}/progress`);
      return response.data;
    } catch (error) {
      if ((error as AxiosError).response?.status === 404) {
        return null;
      }
      throw error;
    }
  }

  async updateProgress(mediaId: string, data: ProgressUpdateRequest): Promise<PlaybackState> {
    const response = await this.client.put<PlaybackState>(`/api/media/${mediaId}/progress`, data);
    return response.data;
  }

  // Сериалы
  async getSeriesEpisodes(seriesId: string): Promise<SeriesEpisodesResponse> {
    const response = await this.client.get<SeriesEpisodesResponse>(`/api/series/${seriesId}/episodes`);
    return response.data;
  }
}

export const apiClient = new ApiClient();

