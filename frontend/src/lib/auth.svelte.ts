import { apiClient } from '../api/client';
import type { LoginRequest, RegisterRequest, AuthResponse } from '../types';

class Auth {
  isAuthenticated = $state(false);
  user = $state<{ id: number; username: string } | null>(null);
  loading = $state(true);

  constructor() {
    // Проверяем наличие токена при загрузке.
    // Валидность не проверяем: первый же 401 сбросит токен и вернёт на логин
    if (localStorage.getItem('auth_token')) {
      this.isAuthenticated = true;
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
