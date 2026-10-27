//! What travels between two devices, and how a table is kept ready for it.
//!
//! Every synced row carries a `sync_id` — the same on every device, where the local
//! `id` differs — and a `sync_at`, the clock of its last change here (`clock.rs`).
//! Triggers keep both, and write a tombstone when a row is deleted, unless a merge is
//! writing (then the rows keep the other device's clocks). Spec:
//! `docs/superpowers/specs/2026-10-26-iris-sync-design.md`.

/// A table whose rows travel.
#[derive(Debug, Clone, Copy)]
pub struct SyncedTable {
    pub name: &'static str,
    /// Its foreign keys to other synced tables: `(column, table)`. They travel as the
    /// parent's `sync_id`, never as the local id.
    pub parents: &'static [(&'static str, &'static str)],
    /// The columns that name a row on their own on both devices, before any sync:
    /// an account is its address, a tag its name.
    pub natural_key: Option<&'static [&'static str]>,
    /// Only these rows travel: a calendar kept with a server is that server's.
    pub where_local: Option<&'static str>,
    /// Columns that mean something only here: a task's mail thread, a calendar's
    /// last fetch.
    pub local_columns: &'static [&'static str],
    /// The primary key, for the tables without a rowid.
    pub without_rowid_key: Option<&'static [&'static str]>,
}

const fn t(name: &'static str) -> SyncedTable {
    SyncedTable {
        name,
        parents: &[],
        natural_key: None,
        where_local: None,
        local_columns: &[],
        without_rowid_key: None,
    }
}

/// The synced tables, parents before children.
pub const SYNCED: &[SyncedTable] = &[
    t("areas"),
    SyncedTable {
        parents: &[("area_id", "areas")],
        ..t("task_lists")
    },
    SyncedTable {
        parents: &[("area_id", "areas")],
        ..t("goals")
    },
    SyncedTable {
        parents: &[
            ("list_id", "task_lists"),
            ("parent_id", "tasks"),
            ("goal_id", "goals"),
        ],
        local_columns: &["thread_id", "reminded"],
        ..t("tasks")
    },
    SyncedTable {
        parents: &[("goal_id", "goals")],
        ..t("goal_entries")
    },
    SyncedTable {
        parents: &[("goal_id", "goals")],
        ..t("goal_milestones")
    },
    SyncedTable {
        parents: &[("goal_id", "goals"), ("area_id", "areas")],
        local_columns: &["reminded_day"],
        ..t("habits")
    },
    SyncedTable {
        parents: &[("habit_id", "habits")],
        natural_key: Some(&["habit_id", "day"]),
        without_rowid_key: Some(&["habit_id", "day"]),
        ..t("habit_checks")
    },
    SyncedTable {
        parents: &[("area_id", "areas")],
        natural_key: Some(&["area_id", "month"]),
        without_rowid_key: Some(&["area_id", "month"]),
        ..t("area_scores")
    },
    t("cycles"),
    SyncedTable {
        parents: &[("cycle_id", "cycles"), ("goal_id", "goals")],
        natural_key: Some(&["cycle_id", "goal_id"]),
        without_rowid_key: Some(&["cycle_id", "goal_id"]),
        ..t("cycle_goals")
    },
    SyncedTable {
        natural_key: Some(&["id"]),
        ..t("vision")
    },
    SyncedTable {
        where_local: Some("account_id IS NULL"),
        local_columns: &[
            "account_id",
            "remote_href",
            "read_only",
            "etag",
            "last_modified",
            "last_sync",
            "last_error",
        ],
        ..t("calendars")
    },
    SyncedTable {
        parents: &[("calendar_id", "calendars")],
        where_local: Some("calendar_id IN (SELECT id FROM calendars WHERE account_id IS NULL)"),
        local_columns: &["href", "etag", "remote_ics", "dirty"],
        ..t("calendar_events")
    },
    SyncedTable {
        parents: &[("calendar_id", "calendars")],
        natural_key: Some(&["calendar_id", "uid", "occurrence_start"]),
        ..t("event_notes")
    },
    SyncedTable {
        parents: &[("calendar_id", "calendars")],
        natural_key: Some(&["calendar_id", "uid"]),
        ..t("event_colors")
    },
    SyncedTable {
        parents: &[("calendar_id", "calendars")],
        natural_key: Some(&["calendar_id", "uid"]),
        ..t("event_links")
    },
    SyncedTable {
        natural_key: Some(&["uid"]),
        ..t("invite_replies")
    },
    SyncedTable {
        natural_key: Some(&["name"]),
        ..t("account_tags")
    },
    SyncedTable {
        natural_key: Some(&["email"]),
        local_columns: &["last_activity_at"],
        ..t("accounts")
    },
    SyncedTable {
        parents: &[("account_id", "accounts"), ("tag_id", "account_tags")],
        natural_key: Some(&["account_id", "tag_id"]),
        ..t("account_tag_links")
    },
    SyncedTable {
        parents: &[("account_id", "accounts")],
        natural_key: Some(&["account_id", "address"]),
        ..t("account_aliases")
    },
    SyncedTable {
        natural_key: Some(&["id"]),
        ..t("rules")
    },
    SyncedTable {
        natural_key: Some(&["address"]),
        ..t("unsubscribed")
    },
    SyncedTable {
        parents: &[("account_id", "accounts")],
        ..t("scheduled_mail")
    },
];

/// The table of that name, if it travels.
pub fn synced(name: &str) -> Option<&'static SyncedTable> {
    SYNCED.iter().find(|t| t.name == name)
}

/// The columns, backfill and index that make one table travel. Part of migration 28:
/// what it writes must never change once released (a table added later gets its own
/// migration calling these two).
pub(crate) fn columns_for(table: &SyncedTable) -> String {
    let n = table.name;
    format!(
        "ALTER TABLE {n} ADD COLUMN sync_id TEXT;
ALTER TABLE {n} ADD COLUMN sync_at TEXT NOT NULL DEFAULT '';
UPDATE {n} SET sync_id = lower(hex(randomblob(16)));
CREATE UNIQUE INDEX {n}_by_sync_id ON {n}(sync_id);
"
    )
}

/// The triggers that keep a table's `sync_id` and `sync_at`, and its tombstones.
pub(crate) fn triggers_for(table: &SyncedTable, columns: &[String]) -> String {
    let n = table.name;
    // Only what travels: a column that means something here alone (an account's last
    // activity, written at every fetch) changes nothing the other device must hear of.
    let quand = columns
        .iter()
        .map(String::as_str)
        .filter(|c| !table.local_columns.contains(c) && *c != "sync_id" && *c != "sync_at")
        .collect::<Vec<_>>()
        .join(", ");
    let cible = match table.without_rowid_key {
        None => "rowid = NEW.rowid".to_string(),
        Some(cles) => cles
            .iter()
            .map(|c| format!("{c} = NEW.{c}"))
            .collect::<Vec<_>>()
            .join(" AND "),
    };
    format!(
        "CREATE TRIGGER {n}_sync_insert AFTER INSERT ON {n} WHEN iris_merging() = 0 BEGIN
    UPDATE {n} SET sync_id = coalesce(sync_id, lower(hex(randomblob(16)))),
        sync_at = iris_clock() WHERE {cible};
END;
CREATE TRIGGER {n}_sync_update AFTER UPDATE OF {quand} ON {n}
    WHEN iris_merging() = 0 AND NEW.sync_at IS OLD.sync_at BEGIN
    UPDATE {n} SET sync_at = iris_clock() WHERE {cible};
END;
CREATE TRIGGER {n}_sync_delete AFTER DELETE ON {n}
    WHEN iris_merging() = 0 AND OLD.sync_id IS NOT NULL BEGIN
    INSERT OR REPLACE INTO sync_tombstones (tbl, sync_id, sync_at)
        VALUES ('{n}', OLD.sync_id, iris_clock());
END;
"
    )
}
