-- Namespace stored external ids with their provider scheme ("tmdb:123") so
-- multiple metadata providers can coexist without id collisions. All ids
-- written before this migration came from TMDB.
UPDATE _tblmedia
SET external_id = 'tmdb:' || external_id
WHERE external_id IS NOT NULL
  AND external_id NOT LIKE '%:%';

-- Per-library metadata provider ("tmdb" | "tvmaze" | "anilist").
ALTER TABLE library ADD COLUMN provider TEXT NOT NULL DEFAULT 'tmdb';

-- IMDB cross-reference captured from providers when available; unlocks
-- future integrations (subtitles, ratings, trakt) without re-matching.
ALTER TABLE _tblmedia ADD COLUMN imdb_id TEXT;
