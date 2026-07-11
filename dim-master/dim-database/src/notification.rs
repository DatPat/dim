use crate::DatabaseError;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub id: i64,
    pub user_id: Option<i64>,
    pub category: String,
    pub title: String,
    pub body: Option<String>,
    pub media_id: Option<i64>,
    pub poster_path: Option<String>,
    pub created_at: i64,
}

pub struct InsertableNotification {
    pub user_id: Option<i64>,
    pub category: String,
    pub title: String,
    pub body: Option<String>,
    pub media_id: Option<i64>,
    pub poster_path: Option<String>,
}

impl Notification {
    pub async fn insert(
        conn: &mut crate::Transaction<'_>,
        notif: InsertableNotification,
    ) -> Result<Self, DatabaseError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let id = sqlx::query!(
            "INSERT INTO notifications (user_id, category, title, body, media_id, poster_path, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            notif.user_id,
            notif.category,
            notif.title,
            notif.body,
            notif.media_id,
            notif.poster_path,
            now,
        )
        .execute(&mut *conn)
        .await?
        .last_insert_rowid();

        Ok(Self {
            id,
            user_id: notif.user_id,
            category: notif.category,
            title: notif.title,
            body: notif.body,
            media_id: notif.media_id,
            poster_path: notif.poster_path,
            created_at: now,
        })
    }

    /// Get notifications visible to a given user (broadcast + user-specific),
    /// excluding ones the user has already read.
    pub async fn get_unread_for_user(
        conn: &mut crate::Transaction<'_>,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<Self>, DatabaseError> {
        Ok(sqlx::query_as!(
            Self,
            r#"SELECT id, user_id, category, title, body, media_id, poster_path, created_at
               FROM notifications
               WHERE (user_id IS NULL OR user_id = ?)
                 AND id NOT IN (SELECT notification_id FROM notification_reads WHERE user_id = ?)
               ORDER BY created_at DESC
               LIMIT ?"#,
            user_id,
            user_id,
            limit,
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    pub async fn get_unread_count(
        conn: &mut crate::Transaction<'_>,
        user_id: i64,
    ) -> Result<i64, DatabaseError> {
        let row = sqlx::query!(
            r#"SELECT COUNT(*) as "count: i64"
               FROM notifications
               WHERE (user_id IS NULL OR user_id = ?)
                 AND id NOT IN (SELECT notification_id FROM notification_reads WHERE user_id = ?)"#,
            user_id,
            user_id,
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(row.count.unwrap_or(0))
    }

    pub async fn mark_read(
        conn: &mut crate::Transaction<'_>,
        notification_id: i64,
        user_id: i64,
    ) -> Result<(), DatabaseError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        sqlx::query!(
            "INSERT OR IGNORE INTO notification_reads (notification_id, user_id, read_at)
             VALUES (?, ?, ?)",
            notification_id,
            user_id,
            now,
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    pub async fn mark_all_read(
        conn: &mut crate::Transaction<'_>,
        user_id: i64,
    ) -> Result<(), DatabaseError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        sqlx::query!(
            "INSERT OR IGNORE INTO notification_reads (notification_id, user_id, read_at)
             SELECT id, ?, ? FROM notifications
             WHERE (user_id IS NULL OR user_id = ?)
               AND id NOT IN (SELECT notification_id FROM notification_reads WHERE user_id = ?)",
            user_id,
            now,
            user_id,
            user_id,
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Delete notifications older than the given timestamp.
    pub async fn delete_old(
        conn: &mut crate::Transaction<'_>,
        older_than: i64,
    ) -> Result<u64, DatabaseError> {
        let result = sqlx::query!(
            "DELETE FROM notifications WHERE created_at < ?",
            older_than,
        )
        .execute(&mut *conn)
        .await?;
        Ok(result.rows_affected())
    }
}
