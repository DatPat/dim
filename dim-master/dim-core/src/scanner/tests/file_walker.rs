use super::temp_dir;
use std::path::PathBuf;

/// Verify that parse_filenames strips outer parentheses.
#[test]
fn parse_parenthesized_filename() {
    let files = vec![PathBuf::from("/media/Vigilante/Season 1/(Vigilante S01E08).mkv")];
    let parsed = super::super::parse_filenames(files.iter());
    assert_eq!(parsed.len(), 1);

    let (_, metas) = &parsed[0];
    // At least one metadata entry should have the title "Vigilante"
    assert!(
        metas.iter().any(|m| m.name.to_lowercase().contains("vigilante")
            && !m.name.contains('(')),
        "Should extract 'Vigilante' without parens, got: {metas:?}"
    );
    // Should extract season 1, episode 8
    assert!(
        metas.iter().any(|m| m.season == Some(1) && m.episode == Some(8)),
        "Should extract S01E08, got: {metas:?}"
    );
}

/// Verify grandparent directory fallback when parent is "Season N".
#[test]
fn parse_grandparent_fallback() {
    let files = vec![PathBuf::from(
        "/media/You and I Are Polar Opposites/Season 1/[Erai-raws].Seihantai.na.Kimi.to.Boku-04.[1080p].mkv",
    )];
    let parsed = super::super::parse_filenames(files.iter());
    assert_eq!(parsed.len(), 1);

    let (_, metas) = &parsed[0];
    // Grandparent "You and I Are Polar Opposites" should appear as a fallback
    assert!(
        metas.iter().any(|m| m.name == "You and I Are Polar Opposites"),
        "Should have grandparent dir fallback, got: {metas:?}"
    );
}

/// Verify trailing -NN episode numbers are extracted from titles.
#[test]
fn parse_trailing_dash_episode() {
    let files = vec![PathBuf::from(
        "/media/Show/Season 1/[Erai-raws].Seihantai.na.Kimi.to.Boku-04.[1080p].mkv",
    )];
    let parsed = super::super::parse_filenames(files.iter());
    assert_eq!(parsed.len(), 1);

    let (_, metas) = &parsed[0];
    // At least one entry should have the title cleaned (no "-04") and episode=4
    assert!(
        metas.iter().any(|m| !m.name.contains("-04") && m.episode == Some(4)),
        "Should strip -04 from title and extract episode 4, got: {metas:?}"
    );
}

/// Verify trailing -NNvN version suffix is handled (e.g. "-09v2" → episode 9).
#[test]
fn parse_trailing_dash_episode_with_version() {
    let files = vec![PathBuf::from(
        "/media/Demon Slayer/Season 1/[SubsPlease] Kimetsu no Yaiba-09v2 [1080p].mkv",
    )];
    let parsed = super::super::parse_filenames(files.iter());
    assert_eq!(parsed.len(), 1);

    let (_, metas) = &parsed[0];
    // At least one entry should strip -09v2 and extract episode=9
    assert!(
        metas.iter().any(|m| !m.name.contains("-09") && m.episode == Some(9)),
        "Should strip -09v2 from title and extract episode 9, got: {metas:?}"
    );
    // Grandparent fallback should provide "Demon Slayer"
    assert!(
        metas.iter().any(|m| m.name == "Demon Slayer"),
        "Should have grandparent dir fallback, got: {metas:?}"
    );
}

/// Verify that resolution values (1920x1080) are not misinterpreted as season/episode.
#[test]
fn parse_sanitizes_resolution_as_season() {
    let files = vec![PathBuf::from(
        "/media/Gargantia on the Verdurous Planet/Season 1/[Moozzi2] Suisei no Gargantia-10 [BD 1920x1080 x265-10Bit Flac].mkv",
    )];
    let parsed = super::super::parse_filenames(files.iter());
    assert_eq!(parsed.len(), 1);

    let (_, metas) = &parsed[0];
    // No metadata entry should have season=1920 or episode=1080
    assert!(
        !metas.iter().any(|m| m.season == Some(1920)),
        "Should not have season=1920 (resolution misparse), got: {metas:?}"
    );
    assert!(
        !metas.iter().any(|m| m.episode == Some(1080)),
        "Should not have episode=1080 (resolution misparse), got: {metas:?}"
    );
    // Should extract episode=10 from the trailing -10
    assert!(
        metas.iter().any(|m| m.episode == Some(10)),
        "Should extract episode 10 from '-10', got: {metas:?}"
    );
    // Grandparent fallback should have reasonable season/episode
    let gp = metas.iter().find(|m| m.name == "Gargantia on the Verdurous Planet");
    assert!(gp.is_some(), "Should have grandparent fallback, got: {metas:?}");
    let gp = gp.unwrap();
    assert!(
        gp.season.unwrap_or(0) <= 100,
        "Grandparent should not inherit bogus season, got: {gp:?}"
    );
}

/// Verify that hash values in brackets are not misinterpreted as season.
#[test]
fn parse_sanitizes_hash_as_season() {
    let files = vec![PathBuf::from(
        "/media/TRIGUN STAMPEDE/Season 1/[SubsPlease] Trigun Stargaze-16 [1080p] [47218EED].mkv",
    )];
    let parsed = super::super::parse_filenames(files.iter());
    assert_eq!(parsed.len(), 1);

    let (_, metas) = &parsed[0];
    // No metadata entry should have season=47218
    assert!(
        !metas.iter().any(|m| m.season == Some(47218)),
        "Should not have season=47218 (hash misparse), got: {metas:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_walkdir() {
    let tempdir = temp_dir(vec![
        "file1.mkv",
        "file2.avi",
        "file3.txt",
        "a/b/file4.webm",
        "a/file5.mp4",
        ".hidden.mp4",
    ]);

    let mut files = super::super::get_subfiles([tempdir.path()].iter());
    files.sort();

    let mut expected: Vec<PathBuf> =
        IntoIterator::into_iter(["file1.mkv", "file2.avi", "a/b/file4.webm", "a/file5.mp4"])
            .map(|x| tempdir.path().join(x))
            .collect();

    expected.sort();

    assert_eq!(files, expected);
}
