pub use anitomy::Anitomy;
use anitomy::ElementCategory;
pub use torrent_name_parser::Metadata as TorrentMetadata;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Metadata {
    pub name: String,
    pub year: Option<i64>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    /// TMDB language code detected from the filename (e.g. "de-DE" for German releases).
    pub language: Option<String>,
}

/// Normalize a title for TMDB search: dots/underscores to spaces, collapse whitespace, trim,
/// and strip known streaming platform suffixes.
pub fn normalize_title(title: &str) -> String {
    let result = title
        .replace('.', " ")
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    strip_platform_suffix(&result)
}

/// Strip known streaming platform suffixes (e.g. " - ZDFmediathek") from a title.
fn strip_platform_suffix(title: &str) -> String {
    const SUFFIXES: &[&str] = &[
        "zdfmediathek",
        "ardmediathek",
    ];

    let lower = title.to_lowercase();
    for suffix in SUFFIXES {
        // Match patterns like " - ZDFmediathek" or " ZDFmediathek"
        let dash_pattern = format!("- {suffix}");
        if let Some(idx) = lower.rfind(&dash_pattern) {
            return title[..idx].trim().to_string();
        }
        // Also match without dash: "title ZDFmediathek"
        let space_pattern = format!(" {suffix}");
        if lower.ends_with(&space_pattern) {
            return title[..title.len() - space_pattern.len()].trim().to_string();
        }
    }
    title.to_string()
}

/// Detect a language tag in a filename and return the corresponding TMDB language code.
///
/// Scene releases typically include language tags like "German", "French", etc.
/// after the year: `Movie.Title.2024.German.DL.1080p.BluRay.mkv`
pub fn detect_language(filename: &str) -> Option<String> {
    let tokens: Vec<&str> = filename
        .split(|c: char| c == '.' || c == ' ' || c == '_' || c == '-')
        .filter(|t| !t.is_empty())
        .collect();

    for token in &tokens {
        // Match full names and ISO 639-2 abbreviations used in scene releases.
        match token.to_lowercase().as_str() {
            "german" | "deutsch" | "ger" => return Some("de-DE".into()),
            "french" | "francais" | "fre" | "fra" => return Some("fr-FR".into()),
            "spanish" | "espanol" | "spa" => return Some("es-ES".into()),
            "italian" | "italiano" | "ita" => return Some("it-IT".into()),
            "japanese" | "jpn" => return Some("ja-JP".into()),
            "korean" | "kor" => return Some("ko-KR".into()),
            "chinese" | "chi" | "zho" => return Some("zh-CN".into()),
            "russian" | "rus" => return Some("ru-RU".into()),
            "portuguese" | "por" => return Some("pt-BR".into()),
            "dutch" | "dut" | "nld" => return Some("nl-NL".into()),
            "swedish" | "swe" => return Some("sv-SE".into()),
            "danish" | "dan" => return Some("da-DK".into()),
            "norwegian" | "nor" => return Some("no-NO".into()),
            "finnish" | "fin" => return Some("fi-FI".into()),
            "polish" | "pol" => return Some("pl-PL".into()),
            "czech" | "cze" | "ces" => return Some("cs-CZ".into()),
            "hungarian" | "hun" => return Some("hu-HU".into()),
            "romanian" | "rum" | "ron" => return Some("ro-RO".into()),
            "turkish" | "tur" => return Some("tr-TR".into()),
            "greek" | "gre" | "ell" => return Some("el-GR".into()),
            "arabic" | "ara" => return Some("ar-SA".into()),
            "hindi" | "hin" => return Some("hi-IN".into()),
            "thai" | "tha" => return Some("th-TH".into()),
            "vietnamese" | "vie" => return Some("vi-VN".into()),
            _ => {}
        }
    }

    None
}

/// Try to strip a leading release group prefix from a title.
///
/// Scene releases often use the format "groupname-title-quality", which after
/// normalize_title becomes "groupname title quality". If the first word is all
/// lowercase ASCII and ≤12 chars, and the rest is ≥2 words, return the title
/// with the first word removed.
pub fn strip_release_group(title: &str) -> Option<String> {
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.len() < 3 {
        return None;
    }
    let first = words[0];
    // Release groups are typically short, all-lowercase ASCII identifiers.
    if first.len() <= 12
        && first.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && !is_common_word(first)
    {
        Some(words[1..].join(" "))
    } else {
        None
    }
}

/// Returns true if the word is a common English/German article or preposition
/// that is legitimately the first word of a title.
fn is_common_word(w: &str) -> bool {
    matches!(
        w,
        "the" | "a" | "an" | "der" | "die" | "das" | "ein" | "eine"
            | "le" | "la" | "les" | "un" | "une"
            | "el" | "los" | "las"
            | "il" | "lo" | "i" | "gli"
            | "de" | "du" | "des" | "of" | "in" | "on" | "at" | "to"
            | "and" | "or" | "not" | "no" | "my" | "our" | "his" | "her"
    )
}

pub trait FilenameMetadata {
    fn from_str(s: &str) -> Option<Metadata>;
}

/// Safely call `TorrentMetadata::from`, returning `None` instead of panicking.
///
/// The upstream `torrent_name_parser` crate uses `.parse::<i32>().unwrap()` on
/// regex-captured digit strings with no length bound. A filename containing 10+
/// consecutive digits (exceeding i32::MAX) causes a `PosOverflow` panic. We
/// reject such inputs up-front so the parser never sees them.
fn try_tnp_parse(s: &str) -> Option<TorrentMetadata> {
    let mut digit_run: u32 = 0;
    for b in s.bytes() {
        if b.is_ascii_digit() {
            digit_run += 1;
            if digit_run >= 10 {
                return None;
            }
        } else {
            digit_run = 0;
        }
    }
    TorrentMetadata::from(s).ok()
}

impl FilenameMetadata for TorrentMetadata {
    fn from_str(s: &str) -> Option<Metadata> {
        let metadata = try_tnp_parse(s)?;

        Some(Metadata {
            name: normalize_title(metadata.title()),
            year: metadata.year().map(|x| x as i64),
            season: metadata.season().map(|x| x as i64),
            episode: metadata.episode().map(|x| x as i64),
            language: None,
        })
    }
}

impl FilenameMetadata for Anitomy {
    fn from_str(s: &str) -> Option<Metadata> {
        let metadata = match Anitomy::new().parse(s) {
            Ok(v) | Err(v) => v,
        };

        Some(Metadata {
            name: normalize_title(metadata.get(ElementCategory::AnimeTitle)?),
            year: metadata
                .get(ElementCategory::AnimeYear)
                .and_then(|x| x.parse().ok()),
            // If season isnt specified we assume season 1 here.
            season: metadata
                .get(ElementCategory::AnimeSeason)
                .and_then(|x| x.parse().ok())
                .or(Some(1)),
            episode: metadata
                .get(ElementCategory::EpisodeNumber)
                .and_then(|x| x.parse().ok()),
            language: None,
        })
    }
}

/// A special filename metadata extractor that combines torrent_name_parser and anitomy which in
/// some cases is necessary. TNP is really good at extracting show titles but not season and
/// episode numbers. Anitomy excels at this. Here we combine the title extracted by TPN and the
/// season and episode number extracted by Anitomy.
pub struct CombinedExtractor;

impl FilenameMetadata for CombinedExtractor {
    fn from_str(s: &str) -> Option<Metadata> {
        let metadata_tnp = try_tnp_parse(s)?;
        let metadata_anitomy = match Anitomy::new().parse(s) {
            Ok(v) | Err(v) => v,
        };

        Some(Metadata {
            name: normalize_title(metadata_tnp.title()),
            year: metadata_tnp.year().map(|x| x as i64),
            // If season isnt specified we assume season 1 here as some releases only have a
            // episode number and no season number.
            season: metadata_anitomy
                .get(ElementCategory::AnimeSeason)
                .and_then(|x| x.parse().ok())
                .or(Some(1)),
            episode: metadata_anitomy
                .get(ElementCategory::EpisodeNumber)
                .and_then(|x| x.parse().ok()),
            language: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_german_from_scene_release() {
        assert_eq!(
            detect_language("Alles.Steht.Kopf.2.2024.German.DL.EAC3.2160p.WEB.DV.HDR.HEVC-Lansen"),
            Some("de-DE".into())
        );
    }

    #[test]
    fn detect_german_spaced() {
        assert_eq!(
            detect_language("Die Schule der magischen Tiere 3 2024 German DTS 1080p BluRay x264 - EVKAN"),
            Some("de-DE".into())
        );
    }

    #[test]
    fn detect_french() {
        assert_eq!(
            detect_language("Le.Petit.Prince.2015.French.1080p.BluRay"),
            Some("fr-FR".into())
        );
    }

    #[test]
    fn detect_none_for_english() {
        assert_eq!(
            detect_language("Inside.Out.2.2024.1080p.BluRay.x264"),
            None
        );
    }

    #[test]
    fn normalize_strips_zdfmediathek() {
        assert_eq!(
            normalize_title("Die Olchis - Willkommen in Schmuddelfing - ZDFmediathek"),
            "Die Olchis - Willkommen in Schmuddelfing"
        );
    }

    #[test]
    fn normalize_strips_ardmediathek() {
        assert_eq!(
            normalize_title("Some Show - ARDmediathek"),
            "Some Show"
        );
    }

    #[test]
    fn normalize_dots_and_platform() {
        assert_eq!(
            normalize_title("Die.Olchis.Willkommen.in.Schmuddelfing.-.ZDFmediathek"),
            "Die Olchis Willkommen in Schmuddelfing"
        );
    }

    #[test]
    fn normalize_no_platform_unchanged() {
        assert_eq!(
            normalize_title("Inside.Out.2"),
            "Inside Out 2"
        );
    }

    #[test]
    fn detect_jpn_abbreviation() {
        assert_eq!(
            detect_language("Orb.On.the.Movements.of.the.Earth.S01E15.1080p.NF.WEB-DL.JPN.AAC2.0.H.264"),
            Some("ja-JP".into())
        );
    }

    #[test]
    fn detect_ger_abbreviation() {
        assert_eq!(
            detect_language("Movie.2024.GER.DTS.1080p.BluRay"),
            Some("de-DE".into())
        );
    }

    #[test]
    fn strip_release_group_hdmedia() {
        assert_eq!(
            strip_release_group("hdmedia grinch 2018"),
            Some("grinch 2018".into())
        );
    }

    #[test]
    fn strip_release_group_sneakman() {
        assert_eq!(
            strip_release_group("sneakman der wilde roboter"),
            Some("der wilde roboter".into())
        );
    }

    #[test]
    fn strip_release_group_preserves_articles() {
        // "the grinch" starts with "the" — a common word, not a release group
        assert_eq!(strip_release_group("the grinch 2018"), None);
    }

    #[test]
    fn strip_release_group_too_short() {
        // Only 2 words total — don't strip
        assert_eq!(strip_release_group("hdmedia grinch"), None);
    }
}
