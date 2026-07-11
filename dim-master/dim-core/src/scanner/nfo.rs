//! Discovery of explicit provider ids from the filesystem.
//!
//! Libraries migrated from Kodi/Jellyfin/Sonarr/Radarr usually carry exact
//! identity already: `tvshow.nfo` / `movie.nfo` / `<file>.nfo` sidecars with
//! `<uniqueid>` tags, and folder names like "Show (2019) {tmdb-123}" or
//! "[tvdbid-456]". Using these skips fuzzy title matching entirely — no
//! wrong matches, no TMDB search calls.
//!
//! Returned ids are namespaced ("tmdb:123", "imdb:tt42", "tvdb:456"); the
//! TMDB provider resolves foreign schemes through its `/find` endpoint.

use std::path::Path;

/// Cap sidecar reads — NFOs are tiny; anything huge isn't one.
const MAX_NFO_SIZE: u64 = 256 * 1024;

/// Sources we can recognize, in priority order (a tmdb id needs no extra
/// resolution round-trip, imdb/tvdb go through TMDB `/find`).
const SCHEMES: &[&str] = &["tmdb", "imdb", "tvdb"];

/// Look for an explicit provider id for a media file: first in the file /
/// ancestor directory names, then in NFO sidecars next to the file and in up
/// to two ancestor directories.
pub fn discover_external_id(file: &Path) -> Option<String> {
    // 1. Path-component tags ({tmdb-123}, [tvdbid-456], ...).
    let mut component: Option<&Path> = Some(file);
    for _ in 0..4 {
        let Some(current) = component else { break };
        if let Some(name) = current.file_name().and_then(|n| n.to_str()) {
            if let Some(id) = extract_id_tag(name) {
                return Some(id);
            }
        }
        component = current.parent();
    }

    // 2. NFO sidecars.
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    // "<video>.nfo" next to the file.
    candidates.push(file.with_extension("nfo"));
    if let Some(dir) = file.parent() {
        candidates.push(dir.join("movie.nfo"));
        candidates.push(dir.join("tvshow.nfo"));
        if let Some(parent) = dir.parent() {
            candidates.push(parent.join("tvshow.nfo"));
            if let Some(grandparent) = parent.parent() {
                candidates.push(grandparent.join("tvshow.nfo"));
            }
        }
    }

    for candidate in candidates {
        if let Some(id) = read_nfo_id(&candidate) {
            return Some(id);
        }
    }

    None
}

/// Extract an id tag out of a single path component.
///
/// Recognized shapes (case-insensitive): `{tmdb-123}` `[tmdb-123]`
/// `{tmdbid=123}` `[imdbid-tt0903747]` `{tvdb-81189}` etc.
fn extract_id_tag(component: &str) -> Option<String> {
    let lower = component.to_lowercase();

    for scheme in SCHEMES {
        for key in [format!("{scheme}id"), scheme.to_string()] {
            let mut search_from = 0;
            while let Some(pos) = lower[search_from..].find(&key) {
                let start = search_from + pos;
                search_from = start + key.len();

                // Must be introduced by a bracket/brace (Radarr/Sonarr/Plex
                // style) so plain words can't false-positive.
                let preceded_ok = start > 0
                    && matches!(lower.as_bytes()[start - 1], b'{' | b'[' | b'(');
                if !preceded_ok {
                    continue;
                }

                let rest = &lower[start + key.len()..];
                let rest = match rest.strip_prefix(['-', '=', ':', ' ']) {
                    Some(r) => r,
                    None => continue,
                };

                let value: String = if *scheme == "imdb" {
                    rest.chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .collect()
                } else {
                    rest.chars().take_while(|c| c.is_ascii_digit()).collect()
                };

                if valid_value(scheme, &value) {
                    return Some(format!("{scheme}:{value}"));
                }
            }
        }
    }

    None
}

fn valid_value(scheme: &str, value: &str) -> bool {
    match scheme {
        "imdb" => value.starts_with("tt") && value.len() > 3,
        _ => !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()),
    }
}

/// Read a Kodi/Jellyfin-style NFO and extract the best id.
fn read_nfo_id(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_NFO_SIZE {
        return None;
    }

    let content = std::fs::read_to_string(path).ok()?;
    parse_nfo_content(&content)
}

/// Extract an id from NFO XML (without an XML parser — NFOs in the wild are
/// frequently malformed, so tolerant scanning beats strict parsing).
pub(crate) fn parse_nfo_content(content: &str) -> Option<String> {
    let lower = content.to_lowercase();

    // <uniqueid type="tmdb" ...>VALUE</uniqueid> — preferred, scheme-explicit.
    for scheme in SCHEMES {
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find("<uniqueid") {
            let tag_start = search_from + pos;
            let Some(tag_end_rel) = lower[tag_start..].find('>') else {
                break;
            };
            let tag_end = tag_start + tag_end_rel;
            search_from = tag_end + 1;

            let attrs = &lower[tag_start..tag_end];
            if !attrs.contains(&format!("\"{scheme}\"")) && !attrs.contains(&format!("'{scheme}'"))
            {
                continue;
            }

            let value_end = match lower[tag_end + 1..].find('<') {
                Some(rel) => tag_end + 1 + rel,
                None => break,
            };
            let value = lower[tag_end + 1..value_end].trim().to_string();
            if valid_value(scheme, &value) {
                return Some(format!("{scheme}:{value}"));
            }
        }
    }

    // Legacy single-purpose tags: <tmdbid>123</tmdbid>, <imdbid>tt..</imdbid>, <id>tt..</id>
    for (scheme, tag) in [("tmdb", "<tmdbid>"), ("imdb", "<imdbid>"), ("tvdb", "<tvdbid>")] {
        if let Some(pos) = lower.find(tag) {
            let start = pos + tag.len();
            if let Some(end_rel) = lower[start..].find('<') {
                let value = lower[start..start + end_rel].trim().to_string();
                if valid_value(scheme, &value) {
                    return Some(format!("{scheme}:{value}"));
                }
            }
        }
    }

    // themoviedb.org URLs.
    for marker in ["themoviedb.org/movie/", "themoviedb.org/tv/"] {
        if let Some(pos) = lower.find(marker) {
            let start = pos + marker.len();
            let value: String = lower[start..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !value.is_empty() {
                return Some(format!("tmdb:{value}"));
            }
        }
    }

    // Bare imdb ids in an <id> tag or imdb URL.
    if let Some(pos) = lower.find("imdb.com/title/tt") {
        let start = pos + "imdb.com/title/".len();
        let value: String = lower[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if valid_value("imdb", &value) {
            return Some(format!("imdb:{value}"));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_tags() {
        assert_eq!(
            extract_id_tag("Breaking Bad (2008) {tmdb-1396}"),
            Some("tmdb:1396".into())
        );
        assert_eq!(
            extract_id_tag("Breaking Bad [tvdbid-81189]"),
            Some("tvdb:81189".into())
        );
        assert_eq!(
            extract_id_tag("The Matrix (1999) [imdbid-tt0133093]"),
            Some("imdb:tt0133093".into())
        );
        assert_eq!(
            extract_id_tag("Show {tmdbid=456}"),
            Some("tmdb:456".into())
        );
        // No bracket introduction — must not match.
        assert_eq!(extract_id_tag("tmdb-123 the movie"), None);
        assert_eq!(extract_id_tag("Plain Show Name (2020)"), None);
    }

    #[test]
    fn nfo_uniqueid() {
        let nfo = r#"<?xml version="1.0"?>
            <tvshow>
              <title>Breaking Bad</title>
              <uniqueid type="imdb">tt0903747</uniqueid>
              <uniqueid type="tmdb" default="true">1396</uniqueid>
            </tvshow>"#;
        assert_eq!(parse_nfo_content(nfo), Some("tmdb:1396".into()));
    }

    #[test]
    fn nfo_legacy_tags() {
        assert_eq!(
            parse_nfo_content("<movie><tmdbid>603</tmdbid></movie>"),
            Some("tmdb:603".into())
        );
        assert_eq!(
            parse_nfo_content("<movie><imdbid>tt0133093</imdbid></movie>"),
            Some("imdb:tt0133093".into())
        );
    }

    #[test]
    fn nfo_urls() {
        assert_eq!(
            parse_nfo_content("https://www.themoviedb.org/tv/1396-breaking-bad"),
            Some("tmdb:1396".into())
        );
        assert_eq!(
            parse_nfo_content("see https://www.imdb.com/title/tt0133093/"),
            Some("imdb:tt0133093".into())
        );
    }

    #[test]
    fn nfo_garbage() {
        assert_eq!(parse_nfo_content("not xml at all"), None);
        assert_eq!(parse_nfo_content("<uniqueid type=\"tmdb\">"), None);
    }
}
