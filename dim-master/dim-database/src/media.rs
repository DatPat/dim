use crate::library::MediaType;
use crate::DatabaseError;

use serde::Deserialize;
use serde::Serialize;

use tracing::error;

/// Media struct that represents a media object, usually a movie, tv show or a episode of a tv
/// show. This struct is returned by several methods and can be serialized to json.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct Media {
    /// unique id.
    pub id: i64,
    /// id of the library that this media objects belongs to.
    pub library_id: i64,
    /// name of this media object. Usually the title of a movie, episode or tv show.
    pub name: String,
    /// description of this media object. Usually overview of a movie etc.
    pub description: Option<String>,
    /// rating provided by any API that is encoded as a signed integer. Usually TMDB rating.
    pub rating: Option<f64>,
    /// Year in which this movie/tv show/episode was released/aired.
    pub year: Option<i64>,
    /// Date when this media object was created and inserted into the database. Used by several
    /// routes to return sorted lists of medias, based on when they were scanned and inserted into
    /// the db.
    pub added: Option<String>,
    /// Path to the media poster.
    pub poster_path: Option<String>,
    /// Path to the backdrop for this media object.
    pub backdrop_path: Option<String>,
    /// Media type encoded as a string. Either movie/tv/episode or none.
    #[serde(flatten)]
    pub media_type: MediaType,
}

impl PartialEq for Media {
    fn eq(&self, other: &Media) -> bool {
        self.id == other.id
    }
}

impl Media {
    /// Method returns all Media objects associated with a Library. Its exactly the same as
    /// [`Library::get`](dim-database::library::Library::get) except it takes in a Library object instead of a id.
    /// [`Library::get`](dim-database::library::Library::get) is a intermediary to this function, as it calls this
    /// function.
    ///
    /// # Arguments
    /// * `conn` - mutable reference to a sqlx transaction.
    /// * `library_id` - a [`Library`](dim-database::library::Library) id.
    pub async fn get_all(
        conn: &mut crate::Transaction<'_>,
        library_id: i64,
    ) -> Result<Vec<Self>, DatabaseError> {
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT id, library_id, name, description, rating as "rating: _", year, added, poster_path, backdrop_path, media_type as "media_type: _" FROM media WHERE library_id = ? AND NOT media_type = "episode""#,
                library_id
            )
            .fetch_all(&mut *conn)
            .await?)
    }

    /// Method returns a media object based on its id
    ///
    /// # Arguments
    /// * `conn` - mutable reference to a sqlx transaction.
    /// * `req_id` - id of a media that we'd like to match against.
    pub async fn get(conn: &mut crate::Transaction<'_>, id: i64) -> Result<Self, DatabaseError> {
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT id, library_id, name, description, rating as "rating: _", year, added, poster_path, backdrop_path, media_type as "media_type: _" FROM media WHERE id = ?"#,
                id
            )
            .fetch_one(&mut *conn)
            .await?)
    }

    /// Method to get a entry in a library based on name and library
    ///
    /// # Arguments
    /// * `conn` - mutable reference to a sqlx transaction.
    /// * `library_id` - a library id.
    /// * `name` - string slice reference containing the name we would like to filter by.
    pub async fn get_by_name_and_lib(
        conn: &mut crate::Transaction<'_>,
        library_id: i64,
        name: &str,
    ) -> Result<Self, DatabaseError> {
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT id, library_id, name, description, rating as "rating: _", year, added, poster_path, backdrop_path, media_type as "media_type: _" FROM media WHERE library_id = ? AND name = ? AND NOT media_type = "episode""#,
                library_id,
                name,
            )
            .fetch_one(&mut *conn)
            .await?)
    }

    pub async fn get_of_mediafile(
        conn: &mut crate::Transaction<'_>,
        mediafile_id: i64,
    ) -> Result<Self, DatabaseError> {
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT media.id, media.library_id, name, description, rating as "rating: _", year, added, poster_path, backdrop_path, media_type as "media_type: _"
                FROM media
                INNER JOIN mediafile ON mediafile.media_id = media.id
                WHERE mediafile.id = ?"#,
                mediafile_id
            ).fetch_one(&mut *conn).await?)
    }

    /// Method returns the top rated medias
    pub async fn get_top_rated(
        conn: &mut crate::Transaction<'_>,
        limit: i64,
    ) -> Result<Vec<i64>, DatabaseError> {
        Ok(sqlx::query_scalar!(
            r#"SELECT _tblmedia.id
                FROM _tblmedia
                JOIN library ON library.id = _tblmedia.library_id
                WHERE NOT _tblmedia.media_type = "episode" AND NOT library.hidden
                ORDER BY rating DESC
                LIMIT ?"#,
            limit
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    /// Method returns the recently added medias
    pub async fn get_recently_added(
        conn: &mut crate::Transaction<'_>,
        limit: i64,
    ) -> Result<Vec<i64>, DatabaseError> {
        Ok(sqlx::query_scalar!(
            r#"SELECT _tblmedia.id
                FROM _tblmedia
                JOIN library ON library.id = _tblmedia.library_id
                WHERE NOT _tblmedia.media_type = "episode" AND NOT library.hidden
                ORDER BY added DESC
                LIMIT ?"#,
            limit
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    pub async fn get_random_with(
        conn: &mut crate::Transaction<'_>,
        limit: i64,
    ) -> Result<Vec<Self>, DatabaseError> {
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT media.id, media.library_id, media.name, description, rating as "rating: _", year, added, poster_path as "poster_path?", backdrop_path as "backdrop_path?", media.media_type as "media_type: _"
                FROM media
                JOIN library ON media.library_id = library.id
                WHERE NOT media.media_type = "episode" AND NOT library.hidden
                GROUP BY media.id
                ORDER BY RANDOM()
                LIMIT ?
                "#,
                limit
        ).fetch_all(&mut *conn).await?)
    }

    pub async fn get_search(
        conn: &mut crate::Transaction<'_>,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Self>, DatabaseError> {
        let query = format!("%{}%", query);
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT media.id, media.library_id, media.name, description, rating as "rating: _", year, added, poster_path, backdrop_path, media.media_type as "media_type: _"
                FROM media
                JOIN library ON library.id = media.library_id
                WHERE NOT media.media_type = "episode" AND NOT library.hidden
                AND UPPER(media.name) LIKE ?
                LIMIT ?
                "#,
                query,
                limit
        ).fetch_all(&mut *conn).await?)
    }

    pub async fn get_of_genre(
        conn: &mut crate::Transaction<'_>,
        genre_id: i64,
    ) -> Result<Vec<Self>, DatabaseError> {
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT media.id, media.library_id, media.name, description, rating as "rating: _", year, added, poster_path, backdrop_path, media.media_type as "media_type: _"
                FROM media
                INNER JOIN genre_media ON genre_media.media_id = media.id
                JOIN library ON library.id = media.library_id
                WHERE NOT media.media_type = "episode" AND NOT library.hidden
                AND genre_media.genre_id = ?
                "#,
                genre_id,
        ).fetch_all(&mut *conn).await?)
    }

    pub async fn get_of_year(
        conn: &mut crate::Transaction<'_>,
        year: i64,
    ) -> Result<Vec<Self>, DatabaseError> {
        Ok(sqlx::query_as!(
                Media,
                r#"SELECT media.id, media.library_id, media.name, description, rating as "rating: _", year, added, poster_path, backdrop_path, media.media_type as "media_type: _"
                FROM media
                JOIN library ON library.id = media.library_id
                WHERE NOT media.media_type = "episode" AND NOT library.hidden
                AND year = ?
                "#,
                year,
        ).fetch_all(&mut *conn).await?)
    }

    pub async fn get_first_duration(&self, conn: &mut crate::Transaction<'_>) -> i64 {
        sqlx::query!(
            r#"
            SELECT COALESCE(mediafile.duration, 0) as "duration!: i64" FROM mediafile
            WHERE mediafile.media_id = ?
            ORDER BY mediafile.duration DESC
            LIMIT 1
        "#,
            self.id
        )
        .fetch_one(&mut *conn)
        .await
        .map(|x| x.duration)
        .unwrap_or(0)
    }

    pub async fn get_id_by_name(
        tx: &mut crate::Transaction<'_>,
        name: &str,
    ) -> Result<Option<i64>, DatabaseError> {
        Ok(
            sqlx::query!(r#"SELECT id FROM _tblmedia where name = ?"#, name)
                .fetch_optional(&mut *tx)
                .await?
                .map(|x| x.id),
        )
    }

    /// Scoped variant of `get_id_by_name` that mirrors the `media_idx` unique
    /// index — episodes and same-named media in other libraries or of other
    /// types can never be returned.
    pub async fn get_id_by_name_scoped(
        tx: &mut crate::Transaction<'_>,
        library_id: i64,
        name: &str,
        media_type: MediaType,
    ) -> Result<Option<i64>, DatabaseError> {
        Ok(sqlx::query!(
            r#"SELECT id FROM _tblmedia WHERE library_id = ? AND name = ? AND media_type = ?"#,
            library_id,
            name,
            media_type,
        )
        .fetch_optional(&mut *tx)
        .await?
        .map(|x| x.id))
    }

    /// Look a media object up by the metadata provider's id — the stable
    /// identity that survives title/language drift between scans.
    pub async fn get_id_by_external_id(
        tx: &mut crate::Transaction<'_>,
        library_id: i64,
        media_type: MediaType,
        external_id: &str,
    ) -> Result<Option<i64>, DatabaseError> {
        Ok(sqlx::query!(
            r#"SELECT id FROM _tblmedia WHERE library_id = ? AND media_type = ? AND external_id = ?"#,
            library_id,
            media_type,
            external_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .map(|x| x.id))
    }

    pub async fn get_external_id(
        tx: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<Option<String>, DatabaseError> {
        Ok(
            sqlx::query!(r#"SELECT external_id FROM _tblmedia WHERE id = ?"#, id)
                .fetch_optional(&mut *tx)
                .await?
                .and_then(|x| x.external_id),
        )
    }

    pub async fn set_imdb_id(
        tx: &mut crate::Transaction<'_>,
        id: i64,
        imdb_id: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query!(
            r#"UPDATE _tblmedia SET imdb_id = ? WHERE id = ?"#,
            imdb_id,
            id
        )
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    pub async fn set_external_id(
        tx: &mut crate::Transaction<'_>,
        id: i64,
        external_id: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query!(
            r#"UPDATE _tblmedia SET external_id = ? WHERE id = ?"#,
            external_id,
            id
        )
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    /// Returns the current (poster, backdrop) asset ids of a media object.
    pub async fn get_art(
        tx: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<(Option<i64>, Option<i64>), DatabaseError> {
        let row = sqlx::query!(r#"SELECT poster, backdrop FROM _tblmedia WHERE id = ?"#, id)
            .fetch_one(&mut *tx)
            .await?;
        Ok((row.poster, row.backdrop))
    }

    pub async fn media_mediatype(
        conn: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<MediaType, DatabaseError> {
        Ok(sqlx::query_scalar!(
            r#"SELECT media.media_type as "media_type: _" FROM media WHERE media.id = ?"#,
            id
        )
        .fetch_one(&mut *conn)
        .await?)
    }

    pub async fn decouple_mediafiles(
        conn: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<Vec<i64>, DatabaseError> {
        Ok(sqlx::query_scalar!(
            r#"UPDATE mediafile SET media_id = NULL WHERE media_id = ? RETURNING id AS "id: i64""#,
            id
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    /// Method deletes a media object based on its id.
    ///
    /// # Arguments
    /// * `conn` - mutable reference to a sqlx transaction.
    /// * `id` - id of a media object we want to delete
    pub async fn delete(
        conn: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<usize, DatabaseError> {
        Ok(sqlx::query!("DELETE FROM _tblmedia WHERE id = ?", id)
            .execute(&mut *conn)
            .await?
            .rows_affected() as usize)
    }

    /// This function exists because for some reason `CASCADE DELETE` doesnt work with a sqlite
    /// backend. Thus we must manually delete entries when deleting a library.
    pub async fn delete_by_lib_id(
        conn: &mut crate::Transaction<'_>,
        library_id: i64,
    ) -> Result<usize, DatabaseError> {
        Ok(
            sqlx::query!("DELETE FROM _tblmedia WHERE library_id = ?", library_id)
                .execute(&mut *conn)
                .await?
                .rows_affected() as usize,
        )
    }

    /// Get compact representation of a media object. This returns the library_id and
    /// media_type of the media object.
    pub async fn get_compact(
        tx: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<(i64, MediaType), DatabaseError> {
        type Record = (i64, MediaType);

        Ok(sqlx::query_as::<_, Record>(
            r#"SELECT library_id, media_type AS "media_type: _" FROM _tblmedia WHERE id = ?"#,
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?)
    }
}

impl Into<super::tv::TVShow> for Media {
    fn into(self) -> super::tv::TVShow {
        super::tv::TVShow { id: self.id }
    }
}

/// Struct which represents a insertable media object. It is usually used only by the scanners to
/// insert new media objects. It is the same as [`Media`](Media) except it doesnt have the
/// [`id`](Media::id) field.
#[derive(Clone, Default, Debug)]
pub struct InsertableMedia {
    pub library_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub rating: Option<f64>,
    pub year: Option<i64>,
    pub added: String,
    pub poster: Option<i64>,
    pub backdrop: Option<i64>,
    pub media_type: MediaType,
}

impl InsertableMedia {
    /// Method used to insert a new media object. Caller can optionally specify a media id if they
    /// wish to reuse a media object.
    #[tracing::instrument(skip(self, conn), fields(self.name = %self.name, self.library_id = %self.library_id))]
    pub async fn insert(&self, conn: &mut crate::Transaction<'_>) -> Result<i64, DatabaseError> {
        // NOTE: ON CONFLICT is removed as conflicts cant happen because writes are serialized.
        let id = sqlx::query!(
            r#"INSERT INTO _tblmedia (library_id, name, description, rating, year, added, poster, backdrop, media_type)
            VALUES ($1, $2, $3, $4, $5, $6,$7, $8, $9)
            RETURNING id
            "#,
            self.library_id,
            self.name,
            self.description,
            self.rating,
            self.year,
            self.added,
            self.poster,
            self.backdrop,
            self.media_type
        ).fetch_one(&mut *conn).await?.id;

        Ok(id)
    }

    /// Method used to insert a new media object with a specific id.
    ///
    /// # Arguments
    /// * `conn` - mutable reference to a sqlx transaction.
    /// * `id` - Unused ID to use for this media object.
    pub async fn insert_with_id(
        &self,
        conn: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<i64, DatabaseError> {
        sqlx::query!(
            r#"INSERT INTO _tblmedia (id, library_id, name, description, rating, year, added, poster, backdrop, media_type)
            VALUES ($1, $2, $3, $4, $5, $6,$7, $8, $9, $10)
            ON CONFLICT(id) DO UPDATE SET
            id = excluded.id,
            library_id = excluded.library_id,
            name = excluded.name,
            description = excluded.description,
            rating = excluded.rating,
            year = excluded.year,
            added = excluded.added,
            poster = excluded.poster,
            backdrop = excluded.backdrop,
            media_type = excluded.media_type
            "#,
            id,
            self.library_id,
            self.name,
            self.description,
            self.rating,
            self.year,
            self.added,
            self.poster,
            self.backdrop,
            self.media_type
        ).execute(&mut *conn).await?;

        Ok(id)
    }

    /// Method blindly inserts `self` into the database without checking whether a similar entry exists.
    /// This is especially useful for tv shows as they usually have similar metadata with key differences
    /// which are not indexed in the database.
    pub async fn insert_blind(
        &self,
        conn: &mut crate::Transaction<'_>,
    ) -> Result<i64, DatabaseError> {
        Ok(sqlx::query!(
            r#"INSERT INTO _tblmedia (library_id, name, description, rating, year, added, poster, backdrop, media_type)
            VALUES ($1, $2, $3, $4, $5, $6,$7, $8, $9)"#,
            self.library_id,
            self.name,
            self.description,
            self.rating,
            self.year,
            self.added,
            self.poster,
            self.backdrop,
            self.media_type
        ).execute(&mut *conn).await?.last_insert_rowid())
    }

    /// Lazily inserts the media object passed in with the following behavior.
    ///
    /// If the media id exists in the database and has one child, the media id is
    /// reused and the media object passed in is re-inserted. If the media object
    /// has more than one child, we create a new media object and recouple.
    /// FIXME: How will this behave with cross-mediatype rematches??
    ///
    /// Returns a tuple of the id and whether the media object has been in place modified
    pub async fn lazy_insert(&self, tx: &mut crate::Transaction<'_>) -> Result<i64, DatabaseError> {
        self.lazy_insert_with_external_id(tx, None)
            .await
            .map(|(id, _)| id)
    }

    /// Get-or-insert keyed primarily on the metadata provider's id.
    ///
    /// Matching by title string alone created duplicate library entries: the
    /// provider returns different titles for the same show depending on the
    /// search language/year variant, and each variant minted a fresh media row
    /// with its own poster. The provider id is stable across all of them.
    ///
    /// Returns `(id, created)` where `created` indicates a brand-new row —
    /// callers can skip re-inserting show-level assets/genres for existing
    /// rows.
    pub async fn lazy_insert_with_external_id(
        &self,
        tx: &mut crate::Transaction<'_>,
        external_id: Option<&str>,
    ) -> Result<(i64, bool), DatabaseError> {
        let external_id = external_id.filter(|x| !x.is_empty());

        // 1) Identity match — immune to title/language drift.
        if let Some(eid) = external_id {
            if let Some(id) =
                Media::get_id_by_external_id(tx, self.library_id, self.media_type, eid).await?
            {
                // Refresh metadata but keep existing artwork stable — swapping
                // the poster/backdrop on every rescan makes cards flip between
                // provider variants.
                let mut update = UpdateMedia::from(self.clone());
                let (cur_poster, cur_backdrop) = Media::get_art(tx, id).await?;
                if cur_poster.is_some() {
                    update.poster = None;
                }
                if cur_backdrop.is_some() {
                    update.backdrop = None;
                }
                update.update(tx, id).await.map_err(|error| {
                    error!(?error, %id, "Failed to update media metadata");
                    error
                })?;
                return Ok((id, false));
            }
        }

        // 2) Legacy row matched by (library, name, type) — adopt it and stamp
        //    the identity so future scans take path 1.
        if let Some(id) =
            Media::get_id_by_name_scoped(tx, self.library_id, &self.name, self.media_type)
                .await
                .map_err(|error| {
                    error!(?error, %self.name, "Failed to get a media by name");
                    error
                })?
        {
            UpdateMedia::from(self.clone())
                .update(tx, id)
                .await
                .map_err(|error| {
                    error!(?error, %id, "Failed to update media metadata");
                    error
                })?;

            if let Some(eid) = external_id {
                Media::set_external_id(tx, id, eid).await?;
            }

            return Ok((id, false));
        }

        // 3) Brand new media object.
        let id = self.insert(tx).await.map_err(|error| {
            error!(?error, "Failed to insert media object.");
            error
        })?;

        if let Some(eid) = external_id {
            Media::set_external_id(tx, id, eid).await?;
        }

        Ok((id, true))
    }
}

/// Struct which is used when we need to update information about a media object. Same as
/// [`InsertableMedia`](InsertableMedia) except `library_id` cannot be changed and everything field
/// is a `Option<T>`.
#[derive(Clone, Default, Deserialize, Debug)]
pub struct UpdateMedia {
    pub name: Option<String>,
    pub description: Option<String>,
    pub rating: Option<f64>,
    pub year: Option<i64>,
    pub added: Option<String>,
    pub poster: Option<i64>,
    pub backdrop: Option<i64>,
    pub media_type: Option<MediaType>,
}

impl UpdateMedia {
    /// Method used to update the fields of a media object that is in the database using the id of
    /// this object as a discriminator.
    ///
    /// # Arguments
    /// * `conn` - mutable reference to a sqlx transaction.
    /// * `_id` - id of the media object we want to update
    pub async fn update(
        &self,
        conn: &mut crate::Transaction<'_>,
        id: i64,
    ) -> Result<usize, DatabaseError> {
        crate::opt_update!(conn,
            "UPDATE _tblmedia SET name = ? WHERE id = ?" => (self.name, id),
            "UPDATE _tblmedia SET description = ? WHERE id = ?" => (self.description, id),
            "UPDATE _tblmedia SET rating = ? WHERE id = ?" => (self.rating, id),
            "UPDATE _tblmedia SET year = ? WHERE id = ?" => (self.year, id),
            "UPDATE _tblmedia SET added = ? WHERE id = ?" => (self.added, id),
            "UPDATE _tblmedia SET poster = ? WHERE id = ?" => (self.poster, id),
            "UPDATE _tblmedia SET backdrop = ? WHERE id = ?" => (self.backdrop, id),
            "UPDATE _tblmedia SET media_type = ? WHERE id = ?" => (self.media_type, id)
        );

        Ok(1)
    }
}

impl From<InsertableMedia> for UpdateMedia {
    fn from(other: InsertableMedia) -> Self {
        Self {
            description: other.description,
            rating: other.rating,
            poster: other.poster,
            backdrop: other.backdrop,
            ..Default::default()
        }
    }
}
