use std::collections::BTreeSet;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{basename, Ledger};

/// One project as shown on the pre-sync review screen. `name` is the default
/// basename label (and the join key); `label` is the user's rename (defaults to
/// `name`); `excluded` drops the project from every push. Full paths never
/// appear here — only the basename does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewItem {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub excluded: bool,
}

impl Ledger {
    /// Whether the pre-sync review (the consent moment) has been completed. The
    /// push path refuses to send anything until this is true, so no project label
    /// can leave the machine before the user has had a chance to rename/exclude.
    pub fn projects_reviewed(&self) -> rusqlite::Result<bool> {
        let v: i64 = self.conn.query_row(
            "SELECT projects_reviewed FROM sync_state WHERE id = 1",
            [],
            |r| r.get(0),
        )?;
        Ok(v != 0)
    }

    /// The distinct project labels of this machine's own rows, each with any
    /// stored rename/exclude preference. Labels default to the path basename.
    pub fn project_review_items(&self) -> rusqlite::Result<Vec<ReviewItem>> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut stmt = self.conn.prepare("SELECT project_label, project FROM usage_events")?;
        let rows = stmt.query_map([], |r| {
            let label: Option<String> = r.get(0)?;
            let project: String = r.get(1)?;
            Ok(label.unwrap_or_else(|| basename(&project)))
        })?;
        for row in rows {
            names.insert(row?);
        }
        drop(stmt);

        let mut items = Vec::with_capacity(names.len());
        for name in names {
            let pref: Option<(Option<String>, i64)> = self
                .conn
                .query_row(
                    "SELECT synced_label, excluded FROM project_prefs WHERE name = ?1",
                    params![name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (label, excluded) = pref.map(|(l, e)| (l, e != 0)).unwrap_or((None, false));
            items.push(ReviewItem { name, label, excluded });
        }
        Ok(items)
    }

    /// Persist the review decisions and mark the machine reviewed. This is the
    /// consent write: after it, `pending_batch` honours the renames/exclusions and
    /// the first push may proceed.
    pub fn set_project_review(&self, items: &[ReviewItem]) -> rusqlite::Result<()> {
        for item in items {
            let label = item
                .label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != item.name);
            self.conn.execute(
                "INSERT INTO project_prefs (name, synced_label, excluded) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET
                     synced_label = excluded.synced_label, excluded = excluded.excluded",
                params![item.name, label, item.excluded as i64],
            )?;
        }
        self.conn
            .execute("UPDATE sync_state SET projects_reviewed = 1 WHERE id = 1", [])?;
        Ok(())
    }
}
