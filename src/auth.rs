use crate::config::Config;
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

/// Клаймы JWT токена
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub user_id: u64,
    pub username: String,
    pub exp: i64,
}

/// Секретный ключ для JWT (в продакшене должен быть в переменных окружения)
fn get_secret_key(_config: &Config) -> String {
    std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "aurora-secret-key-change-in-production".to_string())
}

/// Хеширует пароль
pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
}

/// Проверяет пароль
pub fn verify_password(password: &str, hash: &str) -> Result<bool, bcrypt::BcryptError> {
    bcrypt::verify(password, hash)
}

/// Создает JWT токен
pub fn create_token(
    user_id: u64,
    username: &str,
    config: &Config,
) -> Result<String, jsonwebtoken::errors::Error> {
    let expiration = Utc::now() + Duration::days(7); // Токен действителен 7 дней

    let claims = Claims {
        user_id,
        username: username.to_string(),
        exp: expiration.timestamp() as i64,
    };

    let secret = get_secret_key(config);
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_ref()),
    )
}

/// Верифицирует JWT токен
pub fn verify_token(token: &str, config: &Config) -> Result<Claims, jsonwebtoken::errors::Error> {
    let secret = get_secret_key(config);
    let validation = Validation::default();

    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_ref()),
        &validation,
    )?;

    Ok(token_data.claims)
}

/// Извлекает токен из заголовка Authorization
pub fn extract_token(auth_header: Option<&axum::http::HeaderValue>) -> Option<String> {
    let header_str = auth_header?.to_str().ok()?;
    header_str.strip_prefix("Bearer ").map(|s| s.to_string())
}
