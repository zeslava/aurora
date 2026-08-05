use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use uuid::Uuid;

/// Время жизни pairing-кода (5 минут).
const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);

/// Одна активная сессия сопряжения устройства.
#[derive(Clone)]
struct PairingEntry {
    code: String,
    created_at: Instant,
    /// Заполняется после подтверждения с авторизованного устройства.
    user: Option<(u64, String)>,
}

/// Результат опроса статуса сопряжения телевизором.
pub enum PollResult {
    Pending,
    Confirmed { user_id: u64, username: String },
    Expired,
}

/// In-memory хранилище pairing-кодов (аналог Jellyfin Quick Connect).
#[derive(Clone, Default)]
pub struct PairingManager {
    inner: Arc<RwLock<HashMap<Uuid, PairingEntry>>>,
}

impl PairingManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Создаёт новую сессию, возвращает `(pairing_id, 6-значный код)`.
    pub async fn start(&self) -> (Uuid, String) {
        let mut map = self.inner.write().await;

        let code = loop {
            let candidate = gen_code();
            let taken = map
                .values()
                .any(|e| e.code == candidate && e.created_at.elapsed() < PAIRING_TTL);
            if !taken {
                break candidate;
            }
        };

        let pairing_id = Uuid::now_v7();
        map.insert(
            pairing_id,
            PairingEntry {
                code: code.clone(),
                created_at: Instant::now(),
                user: None,
            },
        );

        (pairing_id, code)
    }

    /// Привязывает код к пользователю. Возвращает `false`, если код не найден или истёк.
    pub async fn confirm(&self, code: &str, user_id: u64, username: &str) -> bool {
        let mut map = self.inner.write().await;
        for entry in map.values_mut() {
            if entry.code == code && entry.created_at.elapsed() < PAIRING_TTL {
                entry.user = Some((user_id, username.to_string()));
                return true;
            }
        }
        false
    }

    /// Опрос статуса телевизором. Подтверждённая или истёкшая сессия удаляется.
    pub async fn poll(&self, pairing_id: &Uuid) -> PollResult {
        let mut map = self.inner.write().await;
        match map.get(pairing_id) {
            None => PollResult::Expired,
            Some(entry) if entry.created_at.elapsed() >= PAIRING_TTL => {
                map.remove(pairing_id);
                PollResult::Expired
            }
            Some(entry) => match entry.user.clone() {
                Some((user_id, username)) => {
                    map.remove(pairing_id);
                    PollResult::Confirmed { user_id, username }
                }
                None => PollResult::Pending,
            },
        }
    }

    /// Удаляет истёкшие сессии.
    pub async fn cleanup(&self) {
        let mut map = self.inner.write().await;
        map.retain(|_, e| e.created_at.elapsed() < PAIRING_TTL);
    }
}

/// Генерирует случайный 6-значный код из случайного хвоста UUIDv7.
fn gen_code() -> String {
    let n = (Uuid::now_v7().as_u128() % 1_000_000) as u32;
    format!("{:06}", n)
}
