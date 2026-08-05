use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    let stem = file_path.file_stem()?.to_str()?;
    regex::Regex::new(r"(?i)[._\-\s]part[._\-\s]*(\d{1,2})\b")
        .unwrap()
        .captures(stem)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
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
fn episode_pattern() -> regex::Regex {
    regex::Regex::new(r"(?i)\bs[._\-\s]*(\d{1,3})[._\-\s]*e[._\-\s]*(\d{1,3})\b|\b(\d{1,2})[._\-\s]*x[._\-\s]*(\d{1,3})\b").unwrap()
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
    let season_dir = regex::Regex::new(r"(?i)(?:season|сезон)\s*(\d+)").unwrap();
    let season = components
        .iter()
        .find_map(|c| {
            season_dir
                .captures(c)
                .and_then(|c| c[1].parse::<u32>().ok())
        })
        .unwrap_or(1);

    // Номер эпизода — первое число в имени файла
    let episode = regex::Regex::new(r"(\d+)")
        .unwrap()
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
    let spaced_dash = regex::Regex::new(r"\s+-\s+").unwrap().replace_all(raw, " ");

    let class = if spaced_dash.contains(['.', '_', ' ']) {
        r"[._\s]+"
    } else {
        r"[._\-\s]+"
    };

    regex::Regex::new(class)
        .unwrap()
        .replace_all(&spaced_dash, " ")
        .trim()
        .trim_matches('-')
        .to_string()
}

/// Приводит разделители к пробелам и отрезает технические метки релиза
fn clean_title(raw: &str) -> String {
    let normalized = normalize_separators(raw);

    let cut = noise_pattern()
        .find(&normalized)
        .map(|m| m.start())
        .unwrap_or(normalized.len());

    let title = trim_brackets(&normalized[..cut]);

    if title.is_empty() { normalized } else { title }
}

/// Отрезает хвостовую скобочную группу (годы, релиз-группа) и одиночные скобки по краям
fn trim_brackets(raw: &str) -> String {
    let without_tail = regex::Regex::new(r"[\(\[][^\(\)\[\]]*[\)\]]?\s*$")
        .unwrap()
        .replace(raw.trim(), "");

    without_tail
        .trim()
        .trim_matches(['(', '[', ')', ']'])
        .trim()
        .to_string()
}

/// Технические метки релиза (качество, кодеки, источник, озвучка)
fn noise_pattern() -> regex::Regex {
    regex::Regex::new(
        r"(?i)\b(2160p|1080p|720p|480p|4k|uhd|bluray|blu ray|b[dr]rip|dvdrip|dvdscr|dvd|dcprip|web ?dl ?rip|web ?dl|web ?rip|hdrip|hdtv|camrip|tvrip|satrip|iptvrip|remux|repack|proper|x ?264|x ?265|hevc|avc|h 26[45]|xvid|divx|ac3|eac3|dts|truehd|aac|mp3|10bit|hdr|dub|dubbed|sub|subbed|multi|rus|eng)\b",
    )
    .unwrap()
}

/// Парсит имя файла фильма для извлечения названия и года
fn parse_movie_filename(file_name: &str) -> (String, Option<u32>) {
    // Разделители в один пробел: дальше работаем с «человеческой» строкой
    let normalized = normalize_separators(file_name);

    let year_pattern = regex::Regex::new(r"\b(19|20)\d{2}\b").unwrap();
    // Год в самом начале — часть названия («2012», «1917»), а не метка релиза
    let year_match = year_pattern.find_iter(&normalized).find(|m| m.start() > 0);
    let year = year_match.and_then(|m| m.as_str().parse::<u32>().ok());

    // Всё, что идёт после года или первой технической метки, — мусор релиза
    let cut = [
        year_match.map(|m| m.start()),
        noise_pattern().find(&normalized).map(|m| m.start()),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(normalized.len());

    let title = normalized[..cut]
        .trim()
        .trim_matches(['(', '[', ')', ']'])
        .trim();

    let title = if title.is_empty() {
        normalized.clone()
    } else {
        title.to_string()
    };

    (title, year)
}

#[cfg(test)]
mod tests {
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
        ] {
            assert_eq!(
                parse_movie_filename(input),
                (title.to_string(), year),
                "{input}"
            );
        }
    }
}
