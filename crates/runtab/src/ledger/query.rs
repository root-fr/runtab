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
    fn sql_expr(&self) -> &'static str {
        match self {
            // `project_path` alone, independent of attribution, so a
            // `match_kind = 'none'` row still counts toward its project.
            SavedJoin::None => "NULL",
            SavedJoin::Project => {
                "(SELECT SUM(r.saved_tokens) FROM rtk_events r WHERE r.project_path = usage_events.project)"
            }
            // Only attributed rows carry a `(source, session_id)`, so an
            // unmatched rtk row is naturally excluded here (NULL never
            // equals a real session id).
            SavedJoin::Session => {
                "(SELECT SUM(r.saved_tokens) FROM rtk_events r
                    WHERE r.source = usage_events.source AND r.session_id = usage_events.session_id)"
            }
        }
    }
}

impl Ledger {
    pub fn daily(&self) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("substr(ts, 1, 10)", "k ASC", SavedJoin::None)
    }

    pub fn models(&self) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("model", "total_tokens DESC", SavedJoin::None)
    }

    pub fn projects(&self) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("project", "total_tokens DESC", SavedJoin::Project)
    }

    pub fn sessions(&self) -> rusqlite::Result<Vec<AggregateRow>> {
        self.aggregate("session_id", "total_tokens DESC", SavedJoin::Session)
    }

    // `group_expr` and `order` are fixed internal strings (never user input).
    fn aggregate(&self, group_expr: &str, order: &str, saved_join: SavedJoin) -> rusqlite::Result<Vec<AggregateRow>> {
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
             GROUP BY k
             ORDER BY {order}",
            total = super::schema::TOTAL_TOKENS_EXPR,
            saved = saved_join.sql_expr(),
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
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
        })?;
        rows.collect()
    }
}
