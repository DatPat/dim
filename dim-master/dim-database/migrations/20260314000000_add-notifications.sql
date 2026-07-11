CREATE TABLE notifications (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    -- NULL means broadcast to all users
    user_id INTEGER,
    -- "media_added", "test", etc.
    category TEXT NOT NULL DEFAULT 'media_added',
    title TEXT NOT NULL,
    body TEXT,
    -- optional link to a media entry
    media_id INTEGER,
    poster_path TEXT,
    created_at INTEGER NOT NULL,

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (media_id) REFERENCES _tblmedia(id) ON DELETE SET NULL
);

CREATE INDEX notifications_created_idx ON notifications(created_at DESC);
CREATE INDEX notifications_user_idx ON notifications(user_id);

-- Tracks which notifications each user has dismissed
CREATE TABLE notification_reads (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    notification_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    read_at INTEGER NOT NULL,

    FOREIGN KEY (notification_id) REFERENCES notifications(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX notification_reads_idx ON notification_reads(notification_id, user_id);
