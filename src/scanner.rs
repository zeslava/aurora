use anyhow::{Context, Result};
use chrono::Datelike;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use walkdir::WalkDir;

/// Поддерживаемые расширения видеофайлов
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "ts"];

/// Найденный фильм
pub struct ScannedMovie {
    pub title: String,
    pub year: Option<u32>,
    pub file_path: PathBuf,
}

/// Найденный эпизод сериала
pub struct ScannedEpisode {
    pub season_number: u32,
    pub episode_number: u32,
    pub title: Option<String>,
    pub file_path: PathBuf,
}

/// Найденный сериал с его эпизодами
pub struct ScannedSeries {
    pub title: String,
    /// Год известен только у многосерийных релизов, собранных из имён файлов:
    /// у сериала по директориям сезонов года в имени обычно нет
    pub year: Option<u32>,
    pub episodes: Vec<ScannedEpisode>,
}

/// Результат сканирования директории
pub struct ScanResult {
    pub movies: Vec<ScannedMovie>,
    pub series: Vec<ScannedSeries>,
}

/// Сканирует директорию и группирует найденные файлы в фильмы и сериалы
pub fn scan_directory(path: &Path) -> Result<ScanResult> {
    let mut movies = Vec::new();
    // Группируем эпизоды по названию сериала, сохраняя порядок
    let mut series: BTreeMap<String, Vec<ScannedEpisode>> = BTreeMap::new();

    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let file_path = entry.path();

        if !is_video_file(file_path) {
            continue;
        }

        match classify(file_path, path) {
            Ok(Classified::Movie(movie)) => movies.push(movie),
            Ok(Classified::Episode {
                series_title,
                episode,
            }) => {
                series.entry(series_title).or_default().push(episode);
            }
            Err(e) => tracing::warn!("Ошибка при обработке файла {:?}: {}", file_path, e),
        }
    }

    let (movies, multipart) = split_multipart_movies(movies);

    let mut series: Vec<ScannedSeries> = merge_by_common_prefix(series)
        .into_iter()
        .map(|(title, mut episodes)| {
            episodes.sort_by_key(|e| (e.season_number, e.episode_number));
            ScannedSeries {
                title,
                year: None,
                episodes,
            }
        })
        .collect();
    series.extend(multipart);

    Ok(ScanResult { movies, series })
}

/// Разносит многосерийный релиз, разложенный по файлам «…part.1», «…part.2»,
/// в сериал с эпизодами. У фильма в модели данных ровно один файл, поэтому
/// иначе такой релиз превращается в несколько одинаковых карточек фильма —
/// и TMDB ищет его как фильм, хотя это мини-сериал («The Shining» 1997).
///
/// Группируем по (директория, название, год): один «part 1» без соседей —
/// это просто фильм, у которого номер части попал в имя.
fn split_multipart_movies(movies: Vec<ScannedMovie>) -> (Vec<ScannedMovie>, Vec<ScannedSeries>) {
    let mut groups: BTreeMap<(PathBuf, String, Option<u32>), Vec<(u32, ScannedMovie)>> =
        BTreeMap::new();
    let mut singles = Vec::new();

    for movie in movies {
        match (
            movie.file_path.parent().map(Path::to_path_buf),
            part_number(&movie.file_path),
        ) {
            (Some(dir), Some(part)) => groups
                .entry((dir, movie.title.clone(), movie.year))
                .or_default()
                .push((part, movie)),
            _ => singles.push(movie),
        }
    }

    let mut series = Vec::new();
    for ((_, title, year), mut parts) in groups {
        if parts.len() < 2 {
            singles.extend(parts.into_iter().map(|(_, movie)| movie));
            continue;
        }

        parts.sort_by_key(|(number, _)| *number);
        let episodes = parts
            .into_iter()
            .map(|(number, movie)| ScannedEpisode {
                season_number: 1,
                episode_number: number,
                title: None,
                file_path: movie.file_path,
            })
            .collect();

        series.push(ScannedSeries {
            title,
            year,
            episodes,
        });
    }

    (singles, series)
}

/// Номер части из имени файла: «…part.3», «… Part 3», «…_part3».
/// Перед «part» требуем разделитель явно: `\b` не сработает после `_`,
/// а «_part» — самая частая форма в релизных именах.
fn part_number(file_path: &Path) -> Option<u32> {
    static PART: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)[._\-\s]part[._\-\s]*(\d{1,2})\b").unwrap());
    let stem = file_path.file_stem()?.to_str()?;
    PART.captures(stem)?.get(1)?.as_str().parse().ok()
}

/// Схлопывает сериалы, чьё название целиком является началом другого:
/// «80-e» + «80-e s04 2014» → «80-e». Сравнение пословное, поэтому
/// «Star Wars» и «Star Trek» остаются разными сериалами.
fn merge_by_common_prefix(
    series: BTreeMap<String, Vec<ScannedEpisode>>,
) -> BTreeMap<String, Vec<ScannedEpisode>> {
    let words: BTreeMap<String, Vec<String>> = series
        .keys()
        .map(|t| {
            let w = t.split_whitespace().map(title_key).collect::<Vec<_>>();
            (t.clone(), w)
        })
        .collect();

    let mut merged: BTreeMap<String, Vec<ScannedEpisode>> = BTreeMap::new();
    for (title, episodes) in series {
        // Самое короткое название, начало которого совпадает с текущим
        let target = words
            .iter()
            .filter(|(_, w)| words[&title].starts_with(w))
            .min_by_key(|(_, w)| w.len())
            .map(|(t, _)| t.clone())
            .unwrap_or_else(|| title.clone());

        merged.entry(target).or_default().extend(episodes);
    }
    merged
}

/// Проверяет, является ли файл видеофайлом по расширению
fn is_video_file(path: &Path) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };
    let Some(ext) = ext.to_str() else {
        // Имя файла не в UTF-8 (частая причина — файлы созданы не в UTF-8 locale) —
        // без этого лога файл пропадает из сканирования без единой строки в логах.
        tracing::warn!("Файл пропущен: имя не в UTF-8: {}", path.to_string_lossy());
        return false;
    };
    VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

enum Classified {
    Movie(ScannedMovie),
    Episode {
        series_title: String,
        episode: ScannedEpisode,
    },
}

/// Классифицирует файл как фильм или эпизод сериала по структуре пути и имени
fn classify(file_path: &Path, base_path: &Path) -> Result<Classified> {
    let file_name = file_path
        .file_stem()
        .and_then(|n| n.to_str())
        .context("Не удалось получить имя файла")?;

    let relative_path = file_path.strip_prefix(base_path).unwrap_or(file_path);
    let path_str = relative_path.to_string_lossy().to_lowercase();
    let raw_components: Vec<String> = relative_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let components: Vec<String> = raw_components.iter().map(|c| c.to_lowercase()).collect();

    let looks_like_series = path_str.contains("season")
        || path_str.contains("сезон")
        || episode_pattern().is_match(file_name);

    if looks_like_series {
        let series_title = extract_series_title(file_name, &components, &raw_components);
        let (season_number, episode_number) = parse_episode_numbers(file_name, &components);
        return Ok(Classified::Episode {
            series_title,
            episode: ScannedEpisode {
                season_number,
                episode_number,
                title: None,
                file_path: file_path.to_path_buf(),
            },
        });
    }

    let (title, year) = parse_movie_filename(file_name);
    Ok(Classified::Movie(ScannedMovie {
        title,
        year,
        file_path: file_path.to_path_buf(),
    }))
}

/// Регекс паттерна эпизода в имени файла (S01E02, S1E2, s03.e20, 1x02 и т.д.).
/// Между «s», номером и «e» допустимы разделители релизных имён (точка, дефис, подчёркивание).
fn episode_pattern() -> &'static Regex {
    static EPISODE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bs[._\-\s]*(\d{1,3})[._\-\s]*e[._\-\s]*(\d{1,3})\b|\b(\d{1,2})[._\-\s]*x[._\-\s]*(\d{1,3})\b").unwrap()
    });
    &EPISODE
}

/// Извлекает номера сезона и эпизода из имени файла (с запасными вариантами по пути)
fn parse_episode_numbers(file_name: &str, components: &[String]) -> (u32, u32) {
    if let Some(c) = episode_pattern().captures(file_name) {
        // Первая альтернатива: SxxExx; вторая: NxNN
        let season = c
            .get(1)
            .or_else(|| c.get(3))
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1);
        let episode = c
            .get(2)
            .or_else(|| c.get(4))
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1);
        return (season, episode);
    }

    // Сезон из директории вида "Season 01" / "Сезон 1"
    static SEASON_DIR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(?:season|сезон)\s*(\d+)").unwrap());
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)").unwrap());
    let season = components
        .iter()
        .find_map(|c| {
            SEASON_DIR
                .captures(c)
                .and_then(|c| c[1].parse::<u32>().ok())
        })
        .unwrap_or(1);

    // Номер эпизода — первое число в имени файла
    let episode = NUMBER
        .captures(file_name)
        .and_then(|c| c[1].parse::<u32>().ok())
        .unwrap_or(1);

    (season, episode)
}

/// Извлекает название сериала: сначала из имени файла (часть до SxxExx),
/// иначе — из имени директории сериала
fn extract_series_title(
    file_name: &str,
    components: &[String],
    raw_components: &[String],
) -> String {
    // Кандидат из имени файла: часть до SxxExx
    let from_file = episode_pattern()
        .find(file_name)
        .map(|m| clean_title(&file_name[..m.start()]))
        .filter(|t| !t.is_empty());

    // Кандидат из директории сериала: та, что перед «Season N» / «Сезон N», иначе — верхняя
    let from_dir = components
        .iter()
        .position(|c| c.contains("season") || c.contains("сезон"))
        .filter(|&i| i > 0)
        .map(|i| i - 1)
        .or_else(|| (raw_components.len() > 1).then_some(0))
        .map(|i| clean_title(&raw_components[i]))
        .filter(|t| !t.is_empty());

    match (from_file, from_dir) {
        // Если один вариант — префикс другого, берём более короткий (в длинном остался мусор)
        (Some(file), Some(dir)) => {
            let (fk, dk) = (title_key(&file), title_key(&dir));
            if dk.starts_with(&fk) {
                file
            } else if fk.starts_with(&dk) {
                dir
            } else {
                file
            }
        }
        (Some(t), None) | (None, Some(t)) => t,
        (None, None) => "Неизвестный сериал".to_string(),
    }
}

/// Нормализованный ключ названия для сравнения кандидатов
fn title_key(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Приводит разделители к пробелам, сохраняя дефис внутри слова.
///
/// Релизное имя разделяется чем-то одним, поэтому если в строке уже есть точка,
/// подчёркивание или пробел — дефис к разделителям не относится и остаётся частью
/// слова («80-e.s03.e20» → «80-e»). Дефис, окружённый пробелами, — всегда разделитель.
fn normalize_separators(raw: &str) -> String {
    static SPACED_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+-\s+").unwrap());
    static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[._\s]+").unwrap());
    static WITH_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[._\-\s]+").unwrap());
    let spaced_dash = SPACED_DASH.replace_all(raw, " ");

    let separators = if spaced_dash.contains(['.', '_', ' ']) {
        &SEPARATORS
    } else {
        &WITH_DASH
    };

    separators
        .replace_all(&spaced_dash, " ")
        .trim()
        .trim_matches('-')
        .to_string()
}

/// Общая очистка названий фильмов, сериалов и поисковых запросов TMDB.
pub(crate) fn clean_title(raw: &str) -> String {
    parse_movie_filename(raw).0
}

/// Убирает незакрытую группу перед отрезанной меткой, сохраняя смысловые скобки.
fn trim_brackets(raw: &str) -> String {
    let mut open = Vec::new();
    for (index, ch) in raw.char_indices() {
        match ch {
            '(' | '[' | '{' => open.push((index, ch)),
            ')' | ']' | '}'
                if open.last().is_some_and(|&(_, start)| {
                    matches!((start, ch), ('(', ')') | ('[', ']') | ('{', '}'))
                }) =>
            {
                open.pop();
            }
            _ => {}
        }
    }
    let cut = open.first().map(|&(index, _)| index).unwrap_or(raw.len());
    raw[..cut].trim().trim_end_matches('-').trim().to_string()
}

/// Однозначные технические метки: качество, кодеки, источник и формат кадра.
fn noise_pattern() -> &'static Regex {
    static NOISE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(2160[pi]|1080[pi]|720[pi]|576[pi]|480[pi]|4k|uhd|blu[ -]?ray|bd[ -]?remux|b[dr]rip|dvdrip|dvdscr|dvd|dcprip|web[ -]?dl[ -]?rip|web[ -]?dl|web[ -]?rip|hdrip|hdtv|camrip|tvrip|satrip|iptvrip|remux|x[ -]?26[45]|hevc|avc|h[ -]?26[45]|xvid|divx|e[ -]?ac[ -]?3|ac[ -]?3|ddp(?:[ -]?\d)?|dd\+(?:\d)?|dts|truehd|aac|mp3|10[ -]?bit|hdr(?:10)?|dolby[ -]?vision|open[ -]?matte)\b",
        ).unwrap()
    });
    &NOISE
}

fn bracket_pattern() -> &'static Regex {
    static BRACKETS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\([^()]*\)|\[[^\[\]]*\]|\{[^{}]*\}").unwrap());
    &BRACKETS
}

/// Перед годом или технической меткой спорные слова удаляем только прописными,
/// чтобы сохранить названия вроде «The Sub» и «The Dub».
fn weak_suffix_pattern() -> &'static Regex {
    static WEAK_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(?:PROPER|REPACK|MULTI|RUS|ENG|SUB|SUBBED|DUB|DUBBED)(?:[\s\[\](){}-]+(?:PROPER|REPACK|MULTI|RUS|ENG|SUB|SUBBED|DUB|DUBBED))*[\s\[\](){}-]*$").unwrap()
    });
    &WEAK_SUFFIX
}

fn include_weak_suffix(title: &str, cut: usize) -> usize {
    weak_suffix_pattern()
        .find(&title[..cut])
        .filter(|m| title[..m.start()].chars().any(char::is_alphanumeric))
        .map(|m| m.start())
        .unwrap_or(cut)
}

/// После года и внутри скобок метки релиза распознаём независимо от регистра.
fn release_tags_pattern() -> &'static Regex {
    static RELEASE_TAGS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^[\s\[\](){}-]*(?:proper|repack|multi|rus|eng|sub|subbed|dub|dubbed)(?:[\s\[\](){}-]+(?:proper|repack|multi|rus|eng|sub|subbed|dub|dubbed))*[\s\[\](){}-]*$").unwrap()
    });
    &RELEASE_TAGS
}

fn release_cut(title: &str) -> Option<usize> {
    let strong = noise_pattern().find(title).map(|m| m.start());
    let bracket = bracket_pattern()
        .find_iter(title)
        .find(|m| release_tags_pattern().is_match(&m.as_str()[1..m.len() - 1]))
        .map(|m| m.start());
    let cut = strong.into_iter().chain(bracket).min()?;
    Some(include_weak_suffix(title, cut))
}

fn plausible_year(year: u32) -> bool {
    (1888..=chrono::Utc::now().year() as u32 + 1).contains(&year)
}

fn bracket_year(body: &str) -> Option<u32> {
    static YEAR_RANGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*(\d{4})(?:\s*[-–—]\s*(\d{4})?)?\s*$").unwrap());
    let captures = YEAR_RANGE.captures(body)?;
    let year = captures[1].parse().ok()?;
    let valid_end = captures.get(2).is_none_or(|end| {
        end.as_str()
            .parse::<u32>()
            .is_ok_and(|end| plausible_year(end) && end >= year)
    });
    (plausible_year(year) && valid_end).then_some(year)
}

/// Явный год в скобках имеет приоритет. Без скобок нужен хвост релизного имени;
/// берём последний подходящий год, чтобы «Class.of.1999.1990» сохранил число в названии.
fn release_year(
    title: &str,
    release_style: bool,
    technical_cut: Option<usize>,
) -> Option<(usize, u32)> {
    static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{4}\b").unwrap());
    static PART_SUFFIX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^part\s*\d{1,2}$").unwrap());
    let prefix = &title[..technical_cut.unwrap_or(title.len())];
    let groups: Vec<_> = bracket_pattern().find_iter(prefix).collect();
    if let Some(year) = groups.iter().rev().find_map(|group| {
        let year = bracket_year(&group.as_str()[1..group.len() - 1])?;
        prefix[..group.start()]
            .chars()
            .any(char::is_alphanumeric)
            .then_some((group.start(), year))
    }) {
        return Some(year);
    }

    YEAR.find_iter(prefix)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|m| {
            let year = m.as_str().parse().ok()?;
            if !plausible_year(year)
                || !prefix[..m.start()].chars().any(char::is_alphanumeric)
                || groups
                    .iter()
                    .any(|group| group.range().contains(&m.start()))
            {
                return None;
            }
            let suffix = prefix[m.end()..].trim();
            let tags = release_tags_pattern().is_match(suffix);
            let part = PART_SUFFIX.is_match(suffix);
            let context = release_style || technical_cut.is_some() || tags || part;
            (context && (suffix.is_empty() || tags || part)).then_some((m.start(), year))
        })
}

/// Парсит имя файла фильма для извлечения названия и года
fn parse_movie_filename(file_name: &str) -> (String, Option<u32>) {
    // Разделители в один пробел: дальше работаем с «человеческой» строкой
    let normalized = normalize_separators(file_name);

    let technical_cut = release_cut(&normalized);
    let year_match = release_year(&normalized, file_name.contains(['.', '_']), technical_cut);
    let year = year_match.map(|(_, year)| year);

    // Всё, что идёт после года или первой технической метки, — мусор релиза
    let cut = [year_match.map(|(start, _)| start), technical_cut]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(normalized.len());
    let cut = if year_match.is_some() {
        include_weak_suffix(&normalized, cut)
    } else {
        cut
    };

    let title = trim_brackets(&normalized[..cut]);

    let title = if title.is_empty() {
        normalized.clone()
    } else {
        title
    };

    (title, year)
}

#[cfg(test)]
mod tests {
    use chrono::Datelike;

    use super::{
        BTreeMap, PathBuf, ScannedEpisode, ScannedMovie, episode_pattern, extract_series_title,
        merge_by_common_prefix, normalize_separators, parse_episode_numbers, parse_movie_filename,
        split_multipart_movies,
    };

    /// Собирает фильм так же, как это делает `classify`: название и год — из имени файла
    fn scanned_movie(path: &str) -> ScannedMovie {
        let file_path = PathBuf::from(path);
        let stem = file_path.file_stem().unwrap().to_str().unwrap();
        let (title, year) = parse_movie_filename(stem);
        ScannedMovie {
            title,
            year,
            file_path,
        }
    }

    #[test]
    fn multipart_release_becomes_a_series() {
        let dir = "/media/The.Shining.1997.dvdrip_[4.37]_[teko]";
        let movies = vec![
            scanned_movie(&format!("{dir}/The.Shining.1997.dvdrip_part.3.avi")),
            scanned_movie(&format!("{dir}/The.Shining.1997.dvdrip_part.1.avi")),
            scanned_movie(&format!("{dir}/The.Shining.1997.dvdrip_part.2.avi")),
        ];

        let (movies, series) = split_multipart_movies(movies);

        assert!(movies.is_empty());
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].title, "The Shining");
        assert_eq!(series[0].year, Some(1997));
        assert_eq!(
            series[0]
                .episodes
                .iter()
                .map(|e| (e.season_number, e.episode_number))
                .collect::<Vec<_>>(),
            vec![(1, 1), (1, 2), (1, 3)]
        );
    }

    #[test]
    fn single_part_and_unrelated_files_stay_movies() {
        let movies = vec![
            // Одна часть без соседей — обычный фильм
            scanned_movie("/media/Some.Movie.2001.part.1.avi"),
            // Разные фильмы в одной директории не склеиваются
            scanned_movie("/media/pack/The.Matrix.1999.part.1.avi"),
            scanned_movie("/media/pack/Se7en.1995.part.1.avi"),
            scanned_movie("/media/pack/Alien.1979.avi"),
        ];

        let (movies, series) = split_multipart_movies(movies);

        assert_eq!(movies.len(), 4);
        assert!(series.is_empty());
    }

    fn series_title(dir: &str, file_name: &str) -> String {
        let raw = vec![dir.to_string(), file_name.to_string()];
        let lower: Vec<String> = raw.iter().map(|c| c.to_lowercase()).collect();
        extract_series_title(file_name, &lower, &raw)
    }

    #[test]
    fn series_title_from_file_and_dir() {
        assert_eq!(
            series_title(
                "Breaking Bad 1 - LostFilm.TV [1080p]",
                "Breaking.Bad.S01E01.1080p.rus.LostFilm.TV"
            ),
            "Breaking Bad"
        );
        // Имя файла без названия — берём директорию
        assert_eq!(series_title("The Wire", "S01E01"), "The Wire");
        // Годы в скобках в имени директории не должны попадать в название
        assert_eq!(
            series_title(
                "Scooby Doo Where Are You (1969-1970) DVDRIP RUS",
                "s01e01 - What A Night For A Knight"
            ),
            "Scooby Doo Where Are You"
        );
    }

    #[test]
    fn episode_numbers_with_separators() {
        // Релизные имена вида «80-e.s03.e20» — разделитель между сезоном и эпизодом
        assert!(episode_pattern().is_match("80-e.s03.e20.2013.SATRip"));
        assert_eq!(
            parse_episode_numbers("80-e.s03.e20.2013.SATRip", &[]),
            (3, 20)
        );
        assert_eq!(
            series_title(
                "80-e.s03.2013.SATRip.by.ivandubskoj",
                "80-e.s03.e20.2013.SATRip"
            ),
            "80-e"
        );
        // «s» внутри слова не должна давать ложное срабатывание
        assert!(!episode_pattern().is_match("Salems.Lot.2024.1080p"));
        assert!(!episode_pattern().is_match("The.Devil.Wears.Prada.2.2026"));
    }

    #[test]
    fn merges_series_by_common_prefix() {
        let ep = |p: &str| ScannedEpisode {
            season_number: 1,
            episode_number: 1,
            title: None,
            file_path: std::path::PathBuf::from(p),
        };
        let mut input: BTreeMap<String, Vec<ScannedEpisode>> = BTreeMap::new();
        input.insert("80 e".into(), vec![ep("a")]);
        input.insert("80 e s04 2014".into(), vec![ep("b")]);
        input.insert("Star Trek".into(), vec![ep("c")]);
        input.insert("Star Wars".into(), vec![ep("d")]);

        let merged = merge_by_common_prefix(input);
        let keys: Vec<&String> = merged.keys().collect();
        assert_eq!(keys, vec!["80 e", "Star Trek", "Star Wars"]);
        assert_eq!(merged["80 e"].len(), 2);
    }

    #[test]
    fn dash_is_a_separator_only_when_nothing_else_is() {
        // Есть точки — дефис часть слова
        assert_eq!(normalize_separators("80-e.s03.e20"), "80-e s03 e20");
        // Дефис — единственный разделитель
        assert_eq!(
            normalize_separators("Breaking-Bad-S01E01"),
            "Breaking Bad S01E01"
        );
        // Дефис в пробелах — разделитель даже при наличии точек
        assert_eq!(
            normalize_separators("Breaking Bad 1 - LostFilm.TV"),
            "Breaking Bad 1 LostFilm TV"
        );
    }

    #[test]
    fn cleans_release_tags() {
        for (input, title, year) in [
            ("Наблюдающий_2022_WEB-DLRip", "Наблюдающий", Some(2022)),
            (
                "The.Devil.Wears.Prada.2.2026.DCPRip.1080p-SOFCJ",
                "The Devil Wears Prada 2",
                Some(2026),
            ),
            ("Boiling.Point.HDRip.Kubik.v.Kube", "Boiling Point", None),
            (
                "Scooby Doo Where Are You (1969-1970) DVDRIP RUS",
                "Scooby Doo Where Are You",
                Some(1969),
            ),
            ("2012 (2009) 1080p", "2012", Some(2009)),
            (
                "Chronicles of Narnia Prince Caspian 2008 Open Matte",
                "Chronicles of Narnia Prince Caspian",
                Some(2008),
            ),
            (
                "De l'autre cote du periph 2012 Dub",
                "De l'autre cote du periph",
                Some(2012),
            ),
        ] {
            assert_eq!(
                parse_movie_filename(input),
                (title.to_string(), year),
                "{input}"
            );
        }
    }

    #[test]
    fn cleans_web_release_suffixes_from_movies_and_series() {
        for tag in [
            "WEB-DL",
            "web-dl",
            "WEBDL",
            "WEB DL",
            "WEB.DL",
            "WEB_DL",
            "WEB-DLRip",
            "WEB-DL-Rip",
            "WEB-Rip",
        ] {
            let input = format!("The Secret Life of Pets {tag} KP");
            assert_eq!(
                parse_movie_filename(&input),
                ("The Secret Life of Pets".to_string(), None),
                "{input}"
            );
            assert_eq!(series_title(&input, "S01E01"), "The Secret Life of Pets");
            assert_eq!(
                series_title("media", &format!("{input} S01E01")),
                "The Secret Life of Pets"
            );
        }
    }

    #[test]
    fn keeps_title_words_and_hyphens() {
        for input in [
            "Spider-Man Homecoming",
            "The Web",
            "Web of Lies",
            "The WEB-DLish Story",
        ] {
            assert_eq!(parse_movie_filename(input), (input.to_string(), None));
        }
        assert_eq!(
            parse_movie_filename("Spider-Man WEB-DL KP"),
            ("Spider-Man".to_string(), None)
        );
    }

    #[test]
    fn distinguishes_release_years_from_title_numbers() {
        for (input, title, year) in [
            ("Blade Runner 2049", "Blade Runner 2049", None),
            (
                "Blade.Runner.2049.2017.WEB-DL",
                "Blade Runner 2049",
                Some(2017),
            ),
            ("Blade Runner 2049 (2017)", "Blade Runner 2049", Some(2017)),
            ("Wonder Woman 1984", "Wonder Woman 1984", None),
            (
                "Wonder.Woman.1984.(2020).1080p",
                "Wonder Woman 1984",
                Some(2020),
            ),
            ("Class of 1999", "Class of 1999", None),
            ("Class.of.1999.1990.BluRay", "Class of 1999", Some(1990)),
            ("2001 A Space Odyssey", "2001 A Space Odyssey", None),
            (
                "2001.A.Space.Odyssey.1968",
                "2001 A Space Odyssey",
                Some(1968),
            ),
            ("The Matrix 1999 WEB-DL", "The Matrix", Some(1999)),
            ("The Matrix WEB-DL GROUP 2020", "The Matrix", None),
            ("Summer of 1984 Stories", "Summer of 1984 Stories", None),
            ("Movie.2099.WEB-DL", "Movie 2099", None),
            ("Movie.1895.DVDRip", "Movie", Some(1895)),
        ] {
            assert_eq!(parse_movie_filename(input), (title.into(), year), "{input}");
        }
        let next_year = chrono::Utc::now().year() + 1;
        assert_eq!(
            parse_movie_filename(&format!("Movie ({next_year})")),
            ("Movie".into(), Some(next_year as u32))
        );
    }

    #[test]
    fn release_tags_after_year_are_case_insensitive() {
        for tags in [
            "Dub",
            "dub",
            "dUb",
            "DUB",
            "Dubbed",
            "Proper",
            "Repack",
            "Multi",
            "Rus",
            "Eng",
            "Sub",
            "Subbed",
            "Dub Rus Eng",
            "Dub (Eng)",
        ] {
            for separator in [" ", ".", "_"] {
                let input =
                    format!("De l'autre cote du periph 2012 {tags}").replace(' ', separator);
                assert_eq!(
                    parse_movie_filename(&input),
                    ("De l'autre cote du periph".into(), Some(2012)),
                    "{input}"
                );
                assert_eq!(series_title(&input, "S01E01"), "De l'autre cote du periph");
            }
        }
    }

    #[test]
    fn ambiguous_tags_need_release_context() {
        for title in [
            "A Proper Man",
            "Proper",
            "The Sub",
            "Multi",
            "Dub",
            "The Dub",
            "The Proper",
            "Movie 2012 Dub Story",
            "The Repack Story",
        ] {
            assert_eq!(parse_movie_filename(title), (title.into(), None));
            assert_eq!(
                parse_movie_filename(&format!("{title} WEB-DL KP")),
                (title.into(), None),
                "{title}"
            );
        }
        for input in [
            "Movie PROPER WEB-DL GROUP",
            "Movie MULTI RUS ENG 1080p",
            "Movie [RUS ENG]",
            "Movie (DUB)",
            "Movie.2020.PROPER",
            "Movie.PROPER.(2020).WEB-DL",
        ] {
            assert_eq!(parse_movie_filename(input).0, "Movie", "{input}");
        }
    }

    #[test]
    fn preserves_meaningful_brackets() {
        for title in [
            "Birdman (or The Unexpected Virtue of Ignorance)",
            "The Office (US)",
            "Utopia (UK)",
            "Movie [Special Edition]",
        ] {
            assert_eq!(parse_movie_filename(title), (title.into(), None), "{title}");
            assert_eq!(series_title(title, "S01E01"), title);
            assert_eq!(
                parse_movie_filename(&format!("{title} [1080p] GROUP")).0,
                title
            );
            assert_eq!(
                parse_movie_filename(&format!("{title} (2020) WEB-DL")),
                (title.into(), Some(2020))
            );
        }
        assert_eq!(
            series_title("Series (1969-1970) [1080p]", "S01E01"),
            "Series"
        );
    }

    #[test]
    fn recognizes_extended_release_formats() {
        for tag in [
            "Blu-Ray",
            "Blu.Ray",
            "BDRemux",
            "1080i",
            "576p",
            "HDR10+",
            "Dolby Vision",
            "Dolby.Vision",
            "DDP5.1",
            "DD+5.1",
            "H.264",
            "H-265",
            "E-AC-3",
            "Open Matte",
            "open matte",
            "OPEN.MATTE",
            "Open_Matte",
            "Open-Matte",
            "OpenMatte",
            "[Open Matte]",
        ] {
            let input = format!("Movie {tag} GROUP");
            assert_eq!(
                parse_movie_filename(&input),
                ("Movie".into(), None),
                "{input}"
            );
            assert_eq!(series_title(&input, "S01E01"), "Movie", "{input}");
        }
    }
}
