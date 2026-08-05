use crate::config::Config;
use crate::models::MediaType;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const API_BASE: &str = "https://api.themoviedb.org/3";
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";
/// Размер постера (2/3, хватает и для деталей на 1080p)
const POSTER_SIZE: &str = "w500";
/// Размер фонового кадра для страницы деталей
const BACKDROP_SIZE: &str = "w1280";
/// Пауза между запросами к API, чтобы не упереться в rate limit при первом скане
const REQUEST_DELAY: std::time::Duration = std::time::Duration::from_millis(120);

/// Найденное на TMDB описание фильма или сериала
#[derive(Debug, Clone)]
pub struct TmdbMatch {
    pub tmdb_id: u64,
    /// Тип, по которому кандидат реально нашёлся: сканер определяет его по пути,
    /// и для мини-сериала в директории с фильмами он ошибается
    pub media_type: MediaType,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    /// Путь постера на TMDB (например, "/abc.jpg")
    pub poster_path: Option<String>,
    /// Путь фонового кадра на TMDB
    pub backdrop_path: Option<String>,
}

/// Сезон сериала из `tv/{id}`
#[derive(Debug, Clone, Deserialize)]
pub struct TmdbSeason {
    pub season_number: u32,
    pub poster_path: Option<String>,
}

#[derive(Deserialize)]
struct TvDetails {
    seasons: Vec<TmdbSeason>,
}

#[derive(Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

/// Один кандидат из выдачи поиска. Поля фильмов и сериалов называются по-разному
/// (`title`/`name`, `release_date`/`first_air_date`) — сводим их алиасами.
#[derive(Deserialize)]
struct SearchResult {
    id: u64,
    #[serde(alias = "name")]
    title: Option<String>,
    #[serde(alias = "original_name")]
    original_title: Option<String>,
    original_language: Option<String>,
    popularity: Option<f64>,
    #[serde(alias = "first_air_date")]
    release_date: Option<String>,
    overview: Option<String>,
    vote_average: Option<f64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
}

/// Клиент TMDB с кэшированием изображений на диск
#[derive(Clone)]
pub struct TmdbClient {
    http: reqwest::Client,
    api_key: String,
    language: String,
    images_dir: PathBuf,
}

impl TmdbClient {
    /// Создаёт клиент, если задан `TMDB_API_KEY`; иначе фича выключена
    pub fn from_config(config: &Config) -> Option<Self> {
        let api_key = config.tmdb_api_key.clone()?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .ok()?;

        Some(Self {
            http,
            api_key,
            language: config.tmdb_language.clone(),
            images_dir: config.cache_dir.join("images"),
        })
    }

    /// Директория с закэшированными изображениями
    pub fn images_dir(&self) -> &Path {
        &self.images_dir
    }

    /// Ищет фильм или сериал по названию из имени файла
    pub async fn search(
        &self,
        media_type: &MediaType,
        title: &str,
        year: Option<u32>,
    ) -> Result<Option<TmdbMatch>> {
        let query = normalize_query(title);
        if query.is_empty() {
            return Ok(None);
        }

        let other = match media_type {
            MediaType::Movie => MediaType::TvSeries,
            MediaType::TvSeries => MediaType::Movie,
        };

        // Порядок попыток: свой тип с годом → чужой тип с годом → свой тип без года.
        // Чужой тип с годом идёт раньше поиска без года, потому что тип угадан по
        // пути и врёт чаще, чем год: «The Shining 1997» в директории с фильмами —
        // мини-сериал, а поиск по фильмам без года найдёт «Сияние» 1980-го.
        if let Some(year) = year {
            if let Some(found) = self
                .search_typed(media_type, &query, Some(year), year, false)
                .await?
            {
                return Ok(Some(found));
            }
            tokio::time::sleep(REQUEST_DELAY).await;
            // Чужой тип принимаем только при точном совпадении названия: иначе
            // сюда попадёт первый попавшийся сериал того же года.
            if let Some(found) = self
                .search_typed(&other, &query, Some(year), year, true)
                .await?
            {
                return Ok(Some(found));
            }
            tokio::time::sleep(REQUEST_DELAY).await;
        }

        // Год из имени файла часто врёт — последняя попытка идёт без фильтра,
        // но год всё ещё участвует в ранжировании кандидатов
        self.search_typed(media_type, &query, None, year, false)
            .await
    }

    /// Один запрос поиска по конкретному типу. `filter_year` сужает выдачу на
    /// стороне TMDB, `rank_year` — участвует только в ранжировании кандидатов.
    async fn search_typed(
        &self,
        media_type: &MediaType,
        query: &str,
        filter_year: Option<u32>,
        rank_year: impl Into<Option<u32>>,
        require_title_match: bool,
    ) -> Result<Option<TmdbMatch>> {
        let rank_year = rank_year.into();
        let (endpoint, year_param) = match media_type {
            MediaType::Movie => ("search/movie", "year"),
            MediaType::TvSeries => ("search/tv", "first_air_date_year"),
        };

        let mut params: Vec<(&str, String)> = vec![
            ("api_key", self.api_key.clone()),
            ("language", self.language.clone()),
            ("query", query.to_string()),
            ("include_adult", "false".to_string()),
        ];
        if let Some(year) = filter_year {
            params.push((year_param, year.to_string()));
        }

        let response = self
            .http
            .get(format!("{API_BASE}/{endpoint}"))
            .query(&params)
            .send()
            .await
            .context("Запрос к TMDB не удался")?;

        if !response.status().is_success() {
            anyhow::bail!("TMDB ответил статусом {}", response.status());
        }

        let body: SearchResponse = response
            .json()
            .await
            .context("Не удалось разобрать ответ TMDB")?;

        let best = pick_best(body.results, query, rank_year)
            .filter(|r| !require_title_match || matches_title(r, query));

        Ok(best.map(|r| TmdbMatch {
            tmdb_id: r.id,
            media_type: media_type.clone(),
            overview: r.overview.filter(|o| !o.is_empty()),
            rating: r.vote_average.filter(|v| *v > 0.0),
            poster_path: r.poster_path,
            backdrop_path: r.backdrop_path,
        }))
    }

    /// Список сезонов сериала. Отдельный запрос: поиск сезоны не возвращает.
    pub async fn seasons(&self, tv_id: u64) -> Result<Vec<TmdbSeason>> {
        let response = self
            .http
            .get(format!("{API_BASE}/tv/{tv_id}"))
            .query(&[
                ("api_key", self.api_key.as_str()),
                ("language", self.language.as_str()),
            ])
            .send()
            .await
            .context("Запрос сезонов к TMDB не удался")?;

        if !response.status().is_success() {
            anyhow::bail!("TMDB ответил статусом {} на сезоны", response.status());
        }

        let body: TvDetails = response
            .json()
            .await
            .context("Не удалось разобрать сезоны TMDB")?;

        Ok(body.seasons)
    }

    /// Скачивает постер сезона в кэш. Возвращает имя файла для хранения в БД.
    pub async fn cache_season_poster(
        &self,
        tmdb_path: &str,
        series_id: &str,
        season_number: u32,
    ) -> Result<String> {
        self.cache_image(
            tmdb_path,
            POSTER_SIZE,
            &format!("{series_id}-s{season_number}-poster"),
        )
        .await
    }

    /// Скачивает постер в кэш. Возвращает имя файла для хранения в БД.
    pub async fn cache_poster(&self, tmdb_path: &str, media_id: &str) -> Result<String> {
        self.cache_image(tmdb_path, POSTER_SIZE, &format!("{media_id}-poster"))
            .await
    }

    /// Скачивает фоновый кадр в кэш. Возвращает имя файла для хранения в БД.
    pub async fn cache_backdrop(&self, tmdb_path: &str, media_id: &str) -> Result<String> {
        self.cache_image(tmdb_path, BACKDROP_SIZE, &format!("{media_id}-backdrop"))
            .await
    }

    /// Имя файла включает идентификатор картинки на TMDB, поэтому при смене
    /// найденного фильма меняется и имя. Оно попадает в URL (`?v=`), и клиент
    /// не показывает старую обложку из своего кэша (мы отдаём её на неделю).
    async fn cache_image(&self, tmdb_path: &str, size: &str, stem: &str) -> Result<String> {
        let path = Path::new(tmdb_path);
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
        let tmdb_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let file_name = format!("{stem}-{tmdb_stem}.{extension}");

        let bytes = self
            .http
            .get(format!("{IMAGE_BASE}/{size}{tmdb_path}"))
            .send()
            .await
            .context("Не удалось скачать изображение с TMDB")?
            .error_for_status()
            .context("TMDB вернул ошибку на изображение")?
            .bytes()
            .await
            .context("Не удалось прочитать тело изображения")?;

        tokio::fs::create_dir_all(&self.images_dir)
            .await
            .context("Не удалось создать директорию кэша изображений")?;

        // Пишем во временный файл и переименовываем: битый файл не попадёт в кэш,
        // если процесс упадёт посередине записи
        let target = self.images_dir.join(&file_name);
        let temp = self.images_dir.join(format!("{file_name}.part"));
        tokio::fs::write(&temp, &bytes)
            .await
            .context("Не удалось записать изображение в кэш")?;
        tokio::fs::rename(&temp, &target)
            .await
            .context("Не удалось переместить изображение в кэш")?;

        Ok(file_name)
    }
}

/// Выбирает лучшего кандидата из выдачи.
///
/// Первый результат TMDB брать нельзя: API сортирует по популярности, а не так,
/// как выдача на сайте. По запросу «Восьмидесятые» первым идёт турецкий «Seksenler»
/// (у него совпадает только локализованное название), а российский сериал — вторым.
/// Поэтому считаем счёт по совпадениям, а популярность оставляем лишь тай-брейком.
fn pick_best(results: Vec<SearchResult>, query: &str, year: Option<u32>) -> Option<SearchResult> {
    results
        .into_iter()
        .fold(None, |best, candidate| match best {
            Some(best) if rank(&best, query, year) >= rank(&candidate, query, year) => Some(best),
            _ => Some(candidate),
        })
}

/// Совпало ли название кандидата с запросом (оригинальное или локализованное)
fn matches_title(r: &SearchResult, query: &str) -> bool {
    r.original_title.as_deref().is_some_and(|t| same(t, query))
        || r.title.as_deref().is_some_and(|t| same(t, query))
}

/// Ключ сортировки кандидата: (счёт совпадений, популярность).
/// Популярность в `f64` не `Ord`, поэтому сравниваем кортеж вручную через `partial_cmp`.
fn rank(r: &SearchResult, query: &str, year: Option<u32>) -> (i32, f64) {
    let mut score = 0;

    // Оригинальное название весит больше локализованного: локализованное совпадает
    // и у иностранного сериала, которому русский прокат дал такое же имя.
    // Баллы не суммируются: иначе кандидат, у которого оба названия английские,
    // обгоняет тот же фильм с русским прокатным названием («Boiling Point» /
    // «Точка кипения») только потому, что локализации у него нет.
    if r.original_title.as_deref().is_some_and(|t| same(t, query)) {
        score += 3;
    } else if r.title.as_deref().is_some_and(|t| same(t, query)) {
        score += 2;
    }
    // Запрос кириллицей → скорее всего ищем то, что снято на русском
    if is_cyrillic(query) && r.original_language.as_deref() == Some("ru") {
        score += 2;
    }
    if let (Some(year), Some(date)) = (year, r.release_date.as_deref())
        && date.starts_with(&year.to_string())
    {
        score += 2;
    }

    (score, r.popularity.unwrap_or(0.0))
}

/// Сравнение названий без учёта регистра, пунктуации и лишних пробелов
fn same(a: &str, b: &str) -> bool {
    let key = |s: &str| {
        s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    key(a) == key(b)
}

fn is_cyrillic(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{0400}'..='\u{04FF}'))
}

/// Приводит название из имени файла к поисковому запросу: убирает мусор релиза,
/// разделители и лишние пробелы. Названия сериалов приходят из имени директории
/// в нижнем регистре — для TMDB регистр не важен.
fn normalize_query(title: &str) -> String {
    let cleaned = title.replace(['.', '_', '-'], " ");

    let junk = regex::Regex::new(
        r"(?i)\b(1080p|720p|480p|2160p|4k|bluray|blu ray|bdrip|dvdrip|webrip|web dl|hdtv|hdrip|x264|x265|hevc|h 264|h 265|aac|ac3|dts|remux|proper|repack|rus|eng|sub|dub)\b",
    )
    .unwrap();
    let cleaned = junk.replace_all(&cleaned, " ");

    // Скобочные группы релиз-команд и остатки года в скобках
    let brackets = regex::Regex::new(r"[\[\(\{][^\]\)\}]*[\]\)\}]").unwrap();
    let cleaned = brackets.replace_all(&cleaned, " ");

    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_query_strips_release_junk() {
        assert_eq!(
            normalize_query("The.Matrix.1080p.BluRay.x264-GROUP"),
            "The Matrix GROUP"
        );
        assert_eq!(normalize_query("Интерстеллар [2014]"), "Интерстеллар");
        assert_eq!(normalize_query("breaking bad"), "breaking bad");
    }

    /// Разбирает кандидатов из настоящего ответа TMDB
    fn candidates(json: &str) -> Vec<SearchResult> {
        serde_json::from_str::<SearchResponse>(json)
            .unwrap()
            .results
    }

    /// Реальная выдача `search/tv?language=ru-RU&query=Восьмидесятые`:
    /// турецкий «Seksenler» идёт первым и популярнее, но совпадает у него
    /// только локализованное название.
    #[test]
    fn prefers_original_title_over_popularity() {
        let results = candidates(
            r#"{"results":[
                {"id":50666,"name":"Восьмидесятые","original_name":"Seksenler",
                 "original_language":"tr","popularity":10.88,"first_air_date":"2012-01-24"},
                {"id":62163,"name":"Восьмидесятые","original_name":"Восьмидесятые",
                 "original_language":"ru","popularity":4.55,"first_air_date":"2012-01-30"}
            ]}"#,
        );

        assert_eq!(
            pick_best(results, "Восьмидесятые", None).map(|r| r.id),
            Some(62163)
        );
    }

    #[test]
    fn year_breaks_tie_between_same_titles() {
        let results = candidates(
            r#"{"results":[
                {"id":1,"title":"Дюна","original_title":"Дюна","original_language":"ru",
                 "popularity":50.0,"release_date":"2000-12-03"},
                {"id":2,"title":"Дюна","original_title":"Дюна","original_language":"ru",
                 "popularity":10.0,"release_date":"2021-09-15"}
            ]}"#,
        );

        assert_eq!(
            pick_best(results, "Дюна", Some(2021)).map(|r| r.id),
            Some(2)
        );
    }

    /// Ничего не совпало по названию — остаётся порядок TMDB (по популярности)
    #[test]
    fn falls_back_to_popularity() {
        let results = candidates(
            r#"{"results":[
                {"id":1,"title":"Матрица","original_title":"The Matrix",
                 "original_language":"en","popularity":30.0,"release_date":"1999-03-30"},
                {"id":2,"title":"Матрица времени","original_title":"Before I Fall",
                 "original_language":"en","popularity":5.0,"release_date":"2017-03-02"}
            ]}"#,
        );

        assert_eq!(
            pick_best(results, "Матрица 4к", None).map(|r| r.id),
            Some(1)
        );
    }

    /// Реальная выдача `search/movie?query=The Shining` без года: мини-сериал
    /// 1997-го в ней отсутствует, а фильм 1980-го совпадает по названию.
    /// Кандидат чужого типа принимается только при точном совпадении названия.
    #[test]
    fn cross_type_candidate_requires_title_match() {
        let results = candidates(
            r#"{"results":[
                {"id":1867,"name":"Сияние","original_name":"The Shining",
                 "original_language":"en","popularity":6.76,"first_air_date":"1997-04-27"}
            ]}"#,
        );
        assert!(
            pick_best(results, "The Shining", Some(1997))
                .is_some_and(|r| matches_title(&r, "The Shining"))
        );

        let unrelated = candidates(
            r#"{"results":[
                {"id":42,"name":"Другой сериал","original_name":"Another Show",
                 "original_language":"en","popularity":50.0,"first_air_date":"1997-01-01"}
            ]}"#,
        );
        assert!(
            !pick_best(unrelated, "The Shining", Some(1997))
                .is_some_and(|r| matches_title(&r, "The Shining"))
        );
    }

    #[test]
    fn title_match_ignores_case_and_punctuation() {
        assert!(same("Брат-2", "брат 2"));
        assert!(same("The Matrix", "the matrix"));
        assert!(!same("Матрица", "Матрица времени"));
    }
}
