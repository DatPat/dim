use crate::scanner::parse_filenames;

#[test]
fn debug_unmatched_live_filenames() {
    let files = [
        "/media/tv/An Adventurer's Daily Grind at Age 29/Season 1/[Erai-raws].29 sai.Dokushin.Chuuken.Boukensha.no.Nichijou-06.[1080p.CR.WEB-DL.AVC.AAC][MultiSub][97B0C3E0].mkv",
        "/media/tv/Berserk (2016)/Season 2/[Yellow-Flash] Berserk - 13 [10bit][1080p][26B52C41].mkv",
        "/media/tv/Berserk (2016)/Season 2/[Yellow-Flash].Berserk-23.[10bit][1080p][03C218B5].mkv",
        "/media/tv/Berserk - The Golden Age Arc - Memorial Edition/Season 1/Izu B~1.mkv",
        "/media/tv/DARK MOON - THE BLOOD ALTAR/Season 1/[LbE3L] DARK MOON. THE BLOOD ALTAR S01E04v2 [1080p CR WEBRip AV1 Opus 2.0 Multi-Audio MSubs].mkv",
        "/media/tv/Flower of Evil/Season 1/Agui.kkot.S01E08.Flower.of.Evil.1080p.AMZN.WEB-DL.DD+2.0.H.264-playWEB.mkv",
        "/media/tv/From Old Country Bumpkin to Master Swordsman/Season 1/[Erai-raws].Katainaka.no.Ossan.Kensei.ni.Naru-08.[1080p.AMZN.WEB-DL.AVC.EAC3][MultiSub][8A3AE688].mkv",
        "/media/tv/GOOD BOY/Season 1/NZB 18a9b70c-f419-4962-a072-2b2e54543d16.mkv",
        "/media/tv/JUJUTSU KAISEN/Season 1/[Kaizoku].Jujutsu.Kaisen-22.BD.1080p.[E63F0771].mkv",
        "/media/tv/Mushoku Tensei - Jobless Reincarnation/Season 1/[Moozzi2] Mushoku Tensei Isekai Ittara Honki Dasu-23 END [BD 1920x1080 x265-10Bit Flac].mkv",
        "/media/tv/My Love Story!!/Season 1/[Datte13].My.Love.Story-S01E02.[BD.1080p.Hi10].[Dual-Audio.FLAC].mkv",
        "/media/tv/TRIGUN STAMPEDE/Season 2/[SubsPlease] Trigun Stargaze - 17 (1080p) [4D2EEF39].mkv",
        "/media/tv/TRIGUN STAMPEDE/Season 2/[SubsPlease] Trigun Stargaze-16 [1080p] [47218EED].mkv",
        "/media/tv/The Failed Sage's Academy Domination/Season 1/[SubsPlease] Rakudai Kenja no Gakuin Musou - 01 (1080p) [EFD70185].mkv",
        "/media/tv/Titans (2018)/Season 3/Titans.S03E05.Lazarus.1080p.BluRay.Dts-HDMa5.1.AVC-PiR8.mkv",
    ];

    let parsed = parse_filenames(files.iter());
    for (path, metas) in &parsed {
        println!("== {}", path.display());
        for m in metas {
            println!(
                "   name={:?} year={:?} season={:?} episode={:?}",
                m.name, m.year, m.season, m.episode
            );
        }
    }

    // (name, year, season, episode) tuples that must be present for each file.
    let has = |idx: usize, name: &str, year: Option<i64>, season: i64, episode: i64| {
        assert!(
            parsed[idx].1.iter().any(|m| m.name == name
                && m.year == year
                && m.season == Some(season)
                && m.episode == Some(episode)),
            "file {} missing expected meta {:?}",
            parsed[idx].0.display(),
            name,
        );
    };

    has(0, "An Adventurer's Daily Grind at Age 29", None, 1, 6);
    has(1, "Berserk", Some(2016), 2, 13); // CRC hash must not become the episode
    has(2, "Berserk", Some(2016), 2, 23);
    has(4, "DARK MOON - THE BLOOD ALTAR", None, 1, 4); // v2 suffix
    has(5, "Flower of Evil", None, 1, 8);
    has(6, "From Old Country Bumpkin to Master Swordsman", None, 1, 8); // was ep 688
    has(8, "Jujutsu Kaisen", None, 1, 22); // was ep 63
    has(9, "Mushoku Tensei - Jobless Reincarnation", None, 1, 23); // "-23 END"
    has(10, "My Love Story!!", None, 1, 2); // glued "-S01E02"
    has(11, "TRIGUN STAMPEDE", None, 2, 17);
    has(12, "TRIGUN STAMPEDE", None, 2, 16);
    has(13, "The Failed Sage's Academy Domination", None, 1, 1);
    has(14, "Titans", Some(2018), 3, 5);

    // uuid junk must not poison the folder fallback with fake season/episode
    let good_boy = &parsed[7].1;
    assert!(good_boy
        .iter()
        .any(|m| m.name == "GOOD BOY" && m.season == Some(1) && m.episode.is_none()));
    assert!(!good_boy.iter().any(|m| m.episode == Some(545)));
}
