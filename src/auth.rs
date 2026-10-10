use crate::config::Config;
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Клаймы JWT токена
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub user_id: u64,
    pub username: String,
    pub exp: i64,
}

fn get_secret_key(config: &Config) -> &str {
    &config.jwt_secret
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

/// Адрес из локальной сети. IPv4-mapped IPv6 (`::ffff:192.168.x.x`) приводим к IPv4,
/// иначе при dual-stack сокете LAN-клиенты не распознаются
pub fn is_lan(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// Извлекает токен из заголовка Authorization
pub fn extract_token(auth_header: Option<&axum::http::HeaderValue>) -> Option<String> {
    let header_str = auth_header?.to_str().ok()?;
    header_str.strip_prefix("Bearer ").map(|s| s.to_string())
}
