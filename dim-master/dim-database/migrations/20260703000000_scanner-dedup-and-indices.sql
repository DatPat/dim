-- Track the metadata provider's id (e.g. TMDB id) on each media object so
-- shows/movies are deduplicated by identity rather than by title string.
-- Title-string dedup created duplicate library entries whenever the provider
-- returned a different title for the same show (translated titles, year
-- variants, retitled entries) — the classic "same show twice with two
-- different posters" bug.
ALTER TABLE _tblmedia ADD COLUMN external_id TEXT;
CREATE INDEX media_external_id_idx ON _tblmedia(library_id, media_type, external_id) WHERE external_id IS NOT NULL;

-- Indices for hot scanner/library queries that previously full-scanned:
-- mediafile lookups by parent media and genre_media deletes by media.
CREATE INDEX mediafile_media_id_idx ON mediafile(media_id);
CREATE INDEX genre_media_media_idx ON genre_media(media_id);

-- Clean up orphaned episode media rows leaked by the old scanner (blind
-- inserts whose ids were never linked into the episode table and are not
-- referenced by any mediafile).
DELETE FROM _tblmedia
WHERE media_type = 'episode'
  AND id NOT IN (SELECT id FROM episode)
  AND id NOT IN (SELECT media_id FROM mediafile WHERE media_id IS NOT NULL);
