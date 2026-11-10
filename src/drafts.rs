//! A document's draft history: its state after each save, private to the admin.
//! Restore brings back any kept state; Undo steps back one save at a time. Both
//! are recorded as states of their own, so nothing is ever lost by using them.

use laterite_core::query::{bind_values, build};
use laterite_core::sea_query;
use laterite_core::sqlx;
use laterite_core::{AnyRowExt, Db, DbBackend};
use sea_query::{Alias, Expr, Order, Query};
use sqlx::AnyConnection;

use crate::DOCUMENTS;

/// How many states a document keeps.
pub const KEEP: usize = 50;
pub(crate) const REVISIONS: &str = "legal_draft_revisions";

/// How a kept state came about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A save in the editor.
    Save,
    /// An undo, bringing back the state before.
    Undo,
    /// A restore of a chosen state.
    Restore,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Save => "save",
            Kind::Undo => "undo",
            Kind::Restore => "restore",
        }
    }

    fn parse(name: &str) -> Self {
        match name {
            "undo" => Kind::Undo,
            "restore" => Kind::Restore,
            _ => Kind::Save,
        }
    }
}

/// One kept state of a draft.
#[derive(Debug, Clone)]
pub struct DraftRevision {
    /// The row id.
    pub id: i64,
    /// The title then.
    pub title: String,
    /// The description then.
    pub description: String,
    /// The layout then.
    pub layout_id: Option<i64>,
    /// The draft then.
    pub body: String,
    /// How it came about.
    pub kind: Kind,
    /// For an undo or restore, the state it brought back.
    pub source_id: Option<i64>,
    /// Who.
    pub author: String,
    /// When, RFC 3339 UTC.
    pub created_at: String,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// What the document holds now: title, description, layout, draft.
async fn current(
    conn: &mut AnyConnection,
    backend: DbBackend,
    id: i64,
) -> Result<Option<(String, String, Option<i64>, String)>, sqlx::Error> {
    let (sql, values) = build(
        backend,
        Query::select()
            .columns(["title", "description", "layout_id", "body"].map(Alias::new))
            .from(Alias::new(DOCUMENTS))
            .and_where(Expr::col(Alias::new("id")).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.map(|r| {
        let text = |c: &str| r.get_text_opt(c).ok().flatten().unwrap_or_default();
        (
            text("title"),
            text("description"),
            r.get_int_opt("layout_id").ok().flatten(),
            text("body"),
        )
    }))
}

async fn count(conn: &mut AnyConnection, backend: DbBackend, id: i64) -> Result<i64, sqlx::Error> {
    let (sql, values) = build(
        backend,
        Query::select()
            .expr(Expr::col(Alias::new("id")).count())
            .from(Alias::new(REVISIONS))
            .and_where(Expr::col(Alias::new("document_id")).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_one(&mut *conn)
        .await?;
    Ok(sqlx::Row::try_get::<i64, _>(&row, 0).unwrap_or_default())
}

/// Keeps what document `id` holds now as a state, then drops all but the
/// newest [`KEEP`]. With `baseline`, keeps it only when none is kept yet: the
/// state before the first tracked save.
pub(crate) async fn record_on(
    conn: &mut AnyConnection,
    backend: DbBackend,
    id: i64,
    kind: Kind,
    source: Option<i64>,
    author: &str,
    baseline: bool,
) -> Result<(), sqlx::Error> {
    if baseline && count(conn, backend, id).await? > 0 {
        return Ok(());
    }
    let Some((title, description, layout_id, body)) = current(conn, backend, id).await? else {
        return Ok(());
    };
    let (sql, values) = build(
        backend,
        Query::insert()
            .into_table(Alias::new(REVISIONS))
            .columns(
                [
                    "document_id",
                    "title",
                    "description",
                    "layout_id",
                    "body",
                    "kind",
                    "source_id",
                    "author",
                    "created_at",
                ]
                .map(Alias::new),
            )
            .values_panic([
                id.into(),
                title.into(),
                description.into(),
                layout_id.into(),
                body.into(),
                kind.as_str().into(),
                source.into(),
                author.into(),
                now().into(),
            ])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&mut *conn)
        .await?;
    let old: Vec<i64> = list_on(conn, backend, id)
        .await?
        .into_iter()
        .skip(KEEP)
        .map(|r| r.id)
        .collect();
    if !old.is_empty() {
        let (sql, values) = build(
            backend,
            Query::delete()
                .from_table(Alias::new(REVISIONS))
                .and_where(Expr::col(Alias::new("id")).is_in(old))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// [`record_on`] on a connection of its own.
pub(crate) async fn record(
    db: &Db,
    id: i64,
    kind: Kind,
    source: Option<i64>,
    author: &str,
) -> Result<(), sqlx::Error> {
    let mut conn = db.pool.acquire().await?;
    record_on(&mut conn, db.backend, id, kind, source, author, false).await
}

/// Every kept state of document `id`, newest first.
pub async fn list(db: &Db, id: i64) -> Result<Vec<DraftRevision>, sqlx::Error> {
    let mut conn = db.pool.acquire().await?;
    list_on(&mut conn, db.backend, id).await
}

async fn list_on(
    conn: &mut AnyConnection,
    backend: DbBackend,
    id: i64,
) -> Result<Vec<DraftRevision>, sqlx::Error> {
    let (sql, values) = build(
        backend,
        Query::select()
            .columns(
                [
                    "id",
                    "title",
                    "description",
                    "layout_id",
                    "body",
                    "kind",
                    "source_id",
                    "author",
                    "created_at",
                ]
                .map(Alias::new),
            )
            .from(Alias::new(REVISIONS))
            .and_where(Expr::col(Alias::new("document_id")).eq(id))
            .order_by(Alias::new("id"), Order::Desc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let text = |c: &str| r.get_text_opt(c).ok().flatten().unwrap_or_default();
            DraftRevision {
                id: r.get_int("id").unwrap_or_default(),
                title: text("title"),
                description: text("description"),
                layout_id: r.get_int_opt("layout_id").ok().flatten(),
                body: text("body"),
                kind: Kind::parse(&text("kind")),
                source_id: r.get_int_opt("source_id").ok().flatten(),
                author: text("author"),
                created_at: text("created_at"),
            }
        })
        .collect())
}

/// Writes a kept state back into the document and keeps the result as a state
/// of `kind`.
async fn bring_back(
    db: &Db,
    id: i64,
    rev: &DraftRevision,
    kind: Kind,
    author: &str,
) -> Result<(), sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Alias::new(DOCUMENTS))
            .value(Alias::new("title"), rev.title.clone())
            .value(Alias::new("description"), rev.description.clone())
            .value(Alias::new("layout_id"), rev.layout_id)
            .value(Alias::new("body"), rev.body.clone())
            .value(Alias::new("draft_updated_by"), author)
            .value(Alias::new("updated_at"), now())
            .and_where(Expr::col(Alias::new("id")).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    record(db, id, kind, Some(rev.id), author).await
}

/// Brings back the kept state `revision` of document `id`. `false` when no
/// such state is kept.
pub async fn restore(db: &Db, id: i64, revision: i64, author: &str) -> Result<bool, sqlx::Error> {
    let all = list(db, id).await?;
    let Some(rev) = all.iter().find(|r| r.id == revision) else {
        return Ok(false);
    };
    bring_back(db, id, rev, Kind::Restore, author).await?;
    Ok(true)
}

/// Steps the draft of document `id` back one save: to the state before the
/// latest, or, after an undo, to the state before the one that undo brought
/// back. Returns the state brought back, or `None` with nothing earlier kept.
pub async fn undo(db: &Db, id: i64, author: &str) -> Result<Option<DraftRevision>, sqlx::Error> {
    let all = list(db, id).await?;
    let Some(latest) = all.first() else {
        return Ok(None);
    };
    // Where the draft stands in the history: the latest state, or for a run
    // of undos, the state the last one brought back.
    let here = match latest.kind {
        Kind::Undo => latest
            .source_id
            .and_then(|s| all.iter().position(|r| r.id == s)),
        _ => Some(0),
    };
    let Some(here) = here else { return Ok(None) };
    // The next older state that is a save or a restore, skipping undos.
    let Some(target) = all
        .iter()
        .skip(here + 1)
        .find(|r| r.kind != Kind::Undo)
        .cloned()
    else {
        return Ok(None);
    };
    bring_back(db, id, &target, Kind::Undo, author).await?;
    Ok(Some(target))
}

/// Drops every kept state of a document, with the document.
pub(crate) fn drop_sql(backend: DbBackend, id: i64) -> (String, sea_query::Values) {
    build(
        backend,
        Query::delete()
            .from_table(Alias::new(REVISIONS))
            .and_where(Expr::col(Alias::new("document_id")).eq(id))
            .to_owned(),
    )
}
