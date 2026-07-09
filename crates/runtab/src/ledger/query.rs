use serde::Serialize;

use super::Ledger;

/// One aggregated row for the daily / models / projects / sessions reports.
#[derive(Debug, Serialize)]
pub struct AggregateRow {
    pub key: String,
    pub events: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    /// Events in this group whose model was unknown to the pricing snapshot, so
    /// `cost_usd` omits them — the displayed total is a floor, not the full cost.
    pub unpriced_events: i64,
    /// rtk savings attributed to this group (`projects`/`sessions` only;
    /// `daily`/`models` never populate it). `None` means no rtk row matched
    /// this group at all, distinct from a matched total of zero.
    #[serde(rename = "rtk_saved_tokens", skip_serializing_if = "Option::is_none")]
    pub saved_tokens: Option<i64>,
}

/// Whole-ledger (or since-windowed) rollup for the overview and scan payoff.
#[derive(Debug, Serialize)]
pub struct Totals {
    pub events: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub unpriced_events: i64,
    pub sessions: i64,
    /// Earliest `YYYY-MM-DD` with data in the window; `None` when empty.
    pub first_day: Option<String>,
}

/// How `aggregate`'s `saved_tokens` column is derived, one per report: a
/// scalar correlated subquery evaluated once per output group (not per
/// underlying row — it must never be wrapped in an outer `SUM`, which would
/// re-add the same per-group value once per row and inflate the total).
enum SavedJoin {
    None,
    Project,
    Session,
}

impl SavedJoin {
    // `windowed` mirrors the outer query's `ts >= ?1` bound inside the
    // correlated subquery, so a windowed report never pairs windowed token
    // totals with all-time savings.
    fn sql_expr(&self, windowed: bool) -> String {
        let window = if windowed { " AND r.ts >= ?1" } else { "" };
        match self {
            SavedJoin::None => "NULL".to_string(),
            // `project_path` alone, independent of attribution, so a
            // `match_kind = 'none'` row still counts toward its project.
            SavedJoin::Project => format!(
                "(SELECT SUM(r.saved_tokens) FROM rtk_events r WHERE r.project_path = usage_events.project{window})"
            ),
            // Only attributed rows carry a `(source, session_id)`, so an
            // unmatched rtk row is naturally excluded here (NULL never
            // equals a real session id).
            SavedJoin::Session => format!(
                "(SELECT SUM(r.saved_tokens) FROM rtk_events r
                    WHERE r.source = usage_events.source AND r.session_id = usage_events.session_id{window})"
            ),
        }
    }
}

impl Ledger {
    pub fn daily(&self, since: Option<&str>) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("substr(ts, 1, 10)", "k ASC", SavedJoin::None, since)
    }

    pub fn models(&self, since: Option<&str>) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("model", "total_tokens DESC", SavedJoin::None, since)
    }

    pub fn projects(&self, since: Option<&str>) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("project", "total_tokens DESC", SavedJoin::Project, since)
    }

    pub fn sessions(&self, since: Option<&str>) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("session_id", "total_tokens DESC", SavedJoin::Session, since)
    }

    // `group_expr` and `order` are fixed internal strings (never user input).
    // `since` is a `YYYY-MM-DD` lower bound; RFC 3339 `ts` compares correctly
    // against a bare date prefix.
    fn aggregate(
        &self,
        group_expr: &str,
        order: &str,
        saved_join: SavedJoin,
        since: Option<&str>,
    ) -> rusqlite::Result<Vec<AggregateRow>> {
        let filter = if since.is_some() { "WHERE ts >= ?1" } else { "" };
        let sql = format!(
            "SELECT {group_expr} AS k,
                    COUNT(*) AS events,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM({total}), 0) AS total_tokens,
                    SUM(cost_usd),
                    COALESCE(SUM(cost_usd IS NULL), 0),
                    {saved}
             FROM usage_events
             {filter}
             GROUP BY k
             ORDER BY {order}",
            total = super::schema::TOTAL_TOKENS_EXPR,
            saved = saved_join.sql_expr(since.is_some()),
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row| {
            Ok(AggregateRow {
                key: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                events: r.get(1)?,
                input_tokens: r.get(2)?,
                output_tokens: r.get(3)?,
                cache_read_tokens: r.get(4)?,
                cache_creation_tokens: r.get(5)?,
                total_tokens: r.get(6)?,
                cost_usd: r.get(7)?,
                unpriced_events: r.get(8)?,
                saved_tokens: r.get(9)?,
            })
        };
        match since {
            Some(s) => stmt.query_map([s], map)?.collect(),
            None => stmt.query_map([], map)?.collect(),
        }
    }

    pub fn totals(&self, since: Option<&str>) -> rusqlite::Result<Totals> {
        let filter = if since.is_some() { "WHERE ts >= ?1" } else { "" };
        let sql = format!(
            "SELECT COUNT(*),
                    COALESCE(SUM({total}), 0),
                    SUM(cost_usd),
                    COALESCE(SUM(cost_usd IS NULL), 0),
                    COUNT(DISTINCT session_id),
                    MIN(substr(ts, 1, 10))
             FROM usage_events
             {filter}",
            total = super::schema::TOTAL_TOKENS_EXPR,
        );
        let map = |r: &rusqlite::Row| {
            Ok(Totals {
                events: r.get(0)?,
                total_tokens: r.get(1)?,
                cost_usd: r.get(2)?,
                unpriced_events: r.get(3)?,
                sessions: r.get(4)?,
                first_day: r.get(5)?,
            })
        };
        match since {
            Some(s) => self.conn.query_row(&sql, [s], map),
            None => self.conn.query_row(&sql, [], map),
        }
    }
}
