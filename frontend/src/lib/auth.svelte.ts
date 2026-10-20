import { apiClient } from '../api/client';
import type { LoginRequest, RegisterRequest, AuthResponse } from '../types';

class Auth {
  isAuthenticated = $state(false);
  user = $state<{ id: number; username: string } | null>(null);
  loading = $state(true);
  // Вошли через доверие к домашней сети: вход, выход и сопряжение ТВ не нужны
  lan = $state(false);

  constructor() {
    this.init();
  }

  private async init() {
    // Сначала домашняя сеть — тогда экрана входа нет вовсе.
    // Отказ тут 403, а не 401, так что сохранённый токен не сбрасывается
    if (!(await this.tryLan()) && localStorage.getItem('auth_token')) {
      // Валидность не проверяем: первый же 401 сбросит токен и вернёт на логин
      this.isAuthenticated = true;
      // Ошибку глотаем: невалидный токен и так вернёт на логин через 401-обработчик
      apiClient.refreshToken().then((r) => this.apply(r), () => {});
    }
    this.loading = false;
  }

  private apply(response: AuthResponse) {
    this.isAuthenticated = true;
    this.user = { id: response.user_id, username: response.username };
  }

  async login(data: LoginRequest) {
    this.apply(await apiClient.login(data));
  }

  async tryLan(): Promise<boolean> {
    try {
      this.apply(await apiClient.lanLogin());
      this.lan = true;
      return true;
    } catch {
      return false;
    }
  }

  loginWithResponse(response: AuthResponse) {
    this.apply(response);
  }

  async register(data: RegisterRequest) {
    this.apply(await apiClient.register(data));
  }

  logout() {
    apiClient.clearToken();
    this.isAuthenticated = false;
    this.user = null;
  }
}

export const auth = new Auth();
