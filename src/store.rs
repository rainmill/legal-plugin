//! Documents and their versions: reading them, the version in force on a date,
//! the next number in a document's scheme, and publishing a draft.

use laterite_core::query::{bind_values, build, insert_returning_id};
use laterite_core::{AnyRowExt, Db};
use sea_query::{Alias, Expr, Order, Query};

use crate::{DOCUMENTS, VERSIONS};

/// How a document numbers its versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Numbering {
    /// 1.0, 1.1, 2.0: a material change raises the major number.
    #[default]
    MajorMinor,
    /// The effective date (`2026-11-09`), then `2026-11-09.2` the same day.
    Date,
    /// Whatever the author types.
    Free,
}

impl Numbering {
    /// The stored name.
    pub fn parse(name: &str) -> Self {
        match name {
            "date" => Numbering::Date,
            "free" => Numbering::Free,
            _ => Numbering::MajorMinor,
        }
    }
}

/// A document: its draft and how it is numbered.
#[derive(Debug, Clone)]
pub struct Document {
    /// The row id.
    pub id: i64,
    /// Its address on the site and its key in code (`privacy`).
    pub slug: String,
    /// The title.
    pub title: String,
    /// What it covers.
    pub description: String,
    /// How its versions are numbered.
    pub numbering: Numbering,
    /// Its PDF layout, or the default.
    pub layout_id: Option<i64>,
    /// The draft, markdown.
    pub body: String,
}

/// A published version: never changed once written.
#[derive(Debug, Clone)]
pub struct Version {
    /// The row id.
    pub id: i64,
    /// The document it belongs to.
    pub document_id: i64,
    /// Its number, as published.
    pub number: String,
    /// When it takes force, RFC 3339 UTC.
    pub effective_at: String,
    /// When it was published.
    pub published_at: String,
    /// Who published it.
    pub published_by: String,
    /// What changed.
    pub summary: String,
    /// Whether the change is material.
    pub material: bool,
    /// The text, markdown.
    pub body: String,
    /// BLAKE3 of the body, hex.
    pub content_hash: String,
    /// The PDF stored at publish, a media record.
    pub pdf_media_id: Option<i64>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

const DOCUMENT_COLUMNS: [&str; 7] = [
    "id",
    "slug",
    "title",
    "description",
    "numbering",
    "layout_id",
    "body",
];
const VERSION_COLUMNS: [&str; 11] = [
    "id",
    "document_id",
    "number",
    "effective_at",
    "published_at",
    "published_by",
    "summary",
    "material",
    "body",
    "content_hash",
    "pdf_media_id",
];

fn document_from(r: &sqlx::any::AnyRow) -> Document {
    let text = |c: &str| r.get_text_opt(c).ok().flatten().unwrap_or_default();
    Document {
        id: r.get_int("id").unwrap_or_default(),
        slug: text("slug"),
        title: text("title"),
        description: text("description"),
        numbering: Numbering::parse(&text("numbering")),
        layout_id: r.get_int_opt("layout_id").ok().flatten(),
        body: text("body"),
    }
}

fn version_from(r: &sqlx::any::AnyRow) -> Version {
    let text = |c: &str| r.get_text_opt(c).ok().flatten().unwrap_or_default();
    Version {
        id: r.get_int("id").unwrap_or_default(),
        document_id: r.get_int("document_id").unwrap_or_default(),
        number: text("number"),
        effective_at: text("effective_at"),
        published_at: text("published_at"),
        published_by: text("published_by"),
        summary: text("summary"),
        material: r.get_bool("material").unwrap_or(false),
        body: text("body"),
        content_hash: text("content_hash"),
        pdf_media_id: r.get_int_opt("pdf_media_id").ok().flatten(),
    }
}

/// The document with `id`.
pub async fn document(db: &Db, id: i64) -> Result<Option<Document>, sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(DOCUMENT_COLUMNS.map(Alias::new))
            .from(Alias::new(DOCUMENTS))
            .and_where(Expr::col(Alias::new("id")).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(row.as_ref().map(document_from))
}

/// The document whose address is `slug`.
pub async fn document_by_slug(db: &Db, slug: &str) -> Result<Option<Document>, sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(DOCUMENT_COLUMNS.map(Alias::new))
            .from(Alias::new(DOCUMENTS))
            .and_where(Expr::col(Alias::new("slug")).eq(slug))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(row.as_ref().map(document_from))
}

/// Every version of a document, newest published first.
pub async fn versions(db: &Db, document_id: i64) -> Result<Vec<Version>, sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(VERSION_COLUMNS.map(Alias::new))
            .from(Alias::new(VERSIONS))
            .and_where(Expr::col(Alias::new("document_id")).eq(document_id))
            .order_by(Alias::new("published_at"), Order::Desc)
            .order_by(Alias::new("id"), Order::Desc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows.iter().map(version_from).collect())
}

/// The version in force at `at` (RFC 3339): the one taking force most recently
/// on or before it, the later published winning a tie.
pub fn in_force<'a>(versions: &'a [Version], at: &str) -> Option<&'a Version> {
    versions
        .iter()
        .filter(|v| v.effective_at.as_str() <= at)
        .max_by(|a, b| {
            a.effective_at
                .cmp(&b.effective_at)
                .then(a.published_at.cmp(&b.published_at))
                .then(a.id.cmp(&b.id))
        })
}

/// The number a new version would take: the next major or minor, or the
/// effective date (`date`, `YYYY-MM-DD`) with a suffix when taken. A free
/// scheme proposes nothing.
pub fn next_number(
    numbering: Numbering,
    versions: &[Version],
    material: bool,
    date: &str,
) -> String {
    let taken = |n: &str| versions.iter().any(|v| v.number == n);
    match numbering {
        Numbering::Free => String::new(),
        Numbering::Date => {
            if !taken(date) {
                return date.to_string();
            }
            (2..)
                .map(|n| format!("{date}.{n}"))
                .find(|candidate| !taken(candidate))
                .unwrap_or_default()
        }
        Numbering::MajorMinor => {
            let highest = versions
                .iter()
                .filter_map(|v| {
                    let (major, minor) = v.number.split_once('.')?;
                    Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?))
                })
                .max();
            match highest {
                None => "1.0".into(),
                Some((major, _)) if material => format!("{}.0", major + 1),
                Some((major, minor)) => format!("{major}.{}", minor + 1),
            }
        }
    }
}

/// What a publish needs beyond the draft.
#[derive(Debug, Clone)]
pub struct Publish {
    /// The number; the scheme's next when empty.
    pub number: String,
    /// When it takes force, `YYYY-MM-DD`.
    pub effective: String,
    /// What changed, in plain words.
    pub summary: String,
    /// A material change.
    pub material: bool,
}

/// Why a draft was not published.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// No document has that id.
    #[error("no such document")]
    NotFound,
    /// The draft is empty.
    #[error("the draft is empty")]
    EmptyDraft,
    /// The draft is the text of the latest version.
    #[error("the draft is unchanged since version {0}")]
    Unchanged(String),
    /// No summary of what changed.
    #[error("say what changed")]
    NoSummary,
    /// The effective date did not read as a date.
    #[error("the effective date is not a date (YYYY-MM-DD)")]
    BadDate,
    /// No number was given and the scheme proposes none.
    #[error("give the version a number")]
    NoNumber,
    /// The document already has a version with that number.
    #[error("version {0} already exists")]
    NumberTaken(String),
    /// The database refused.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The text a version holds, as its hash: what was published, provably.
pub fn content_hash(body: &str) -> String {
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

/// Publishes the draft of document `id` as a new version by `author`.
pub async fn publish(
    db: &Db,
    id: i64,
    input: &Publish,
    author: &str,
) -> Result<Version, PublishError> {
    let doc = document(db, id).await?.ok_or(PublishError::NotFound)?;
    if doc.body.trim().is_empty() {
        return Err(PublishError::EmptyDraft);
    }
    if input.summary.trim().is_empty() {
        return Err(PublishError::NoSummary);
    }
    let effective = chrono::NaiveDate::parse_from_str(input.effective.trim(), "%Y-%m-%d")
        .map_err(|_| PublishError::BadDate)?;
    let all = versions(db, id).await?;
    if let Some(latest) = all.first() {
        if latest.body == doc.body {
            return Err(PublishError::Unchanged(latest.number.clone()));
        }
    }
    let number = match input.number.trim() {
        "" => next_number(doc.numbering, &all, input.material, &effective.to_string()),
        given => given.to_string(),
    };
    if number.is_empty() {
        return Err(PublishError::NoNumber);
    }
    if all.iter().any(|v| v.number == number) {
        return Err(PublishError::NumberTaken(number));
    }
    let effective_at = format!("{effective}T00:00:00Z");
    let published_at = now();
    let hash = content_hash(&doc.body);
    let stmt = Query::insert()
        .into_table(Alias::new(VERSIONS))
        .columns(
            [
                "document_id",
                "number",
                "effective_at",
                "published_at",
                "published_by",
                "summary",
                "material",
                "body",
                "content_hash",
            ]
            .map(Alias::new),
        )
        .values_panic([
            id.into(),
            number.clone().into(),
            effective_at.clone().into(),
            published_at.clone().into(),
            author.into(),
            input.summary.trim().into(),
            i32::from(input.material).into(),
            doc.body.clone().into(),
            hash.clone().into(),
        ])
        .to_owned();
    let version_id = insert_returning_id(db, stmt, Alias::new("id")).await?;
    Ok(Version {
        id: version_id,
        document_id: id,
        number,
        effective_at,
        published_at,
        published_by: author.to_string(),
        summary: input.summary.trim().to_string(),
        material: input.material,
        body: doc.body,
        content_hash: hash,
        pdf_media_id: None,
    })
}

/// Records the stored PDF of version `id`.
pub async fn set_pdf(db: &Db, id: i64, media_id: i64) -> Result<(), sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Alias::new(VERSIONS))
            .value(Alias::new("pdf_media_id"), media_id)
            .and_where(Expr::col(Alias::new("id")).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// `2026-11-09T00:00:00Z` as `9 November 2026`.
pub fn long_date(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|d| d.format("%-d %B %Y").to_string())
        .unwrap_or_else(|_| at.chars().take(10).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(number: &str) -> Version {
        Version {
            id: 0,
            document_id: 0,
            number: number.into(),
            effective_at: String::new(),
            published_at: String::new(),
            published_by: String::new(),
            summary: String::new(),
            material: false,
            body: String::new(),
            content_hash: String::new(),
            pdf_media_id: None,
        }
    }

    #[test]
    fn each_scheme_proposes_its_next_number() {
        assert_eq!(
            next_number(Numbering::MajorMinor, &[], false, "2026-11-09"),
            "1.0"
        );
        let some = [v("1.0"), v("1.3"), v("1.2")];
        assert_eq!(next_number(Numbering::MajorMinor, &some, false, ""), "1.4");
        assert_eq!(next_number(Numbering::MajorMinor, &some, true, ""), "2.0");
        assert_eq!(
            next_number(Numbering::Date, &[], false, "2026-11-09"),
            "2026-11-09"
        );
        let today = [v("2026-11-09"), v("2026-11-09.2")];
        assert_eq!(
            next_number(Numbering::Date, &today, false, "2026-11-09"),
            "2026-11-09.3"
        );
        assert_eq!(next_number(Numbering::Free, &some, true, ""), "");
    }

    #[test]
    fn the_version_in_force_is_the_latest_effective_by_then() {
        let mut a = v("1.0");
        a.effective_at = "2026-01-01T00:00:00Z".into();
        let mut b = v("2.0");
        b.effective_at = "2026-12-01T00:00:00Z".into();
        let all = [b.clone(), a.clone()];
        assert_eq!(
            in_force(&all, "2026-11-09T12:00:00Z").unwrap().number,
            "1.0"
        );
        assert_eq!(
            in_force(&all, "2026-12-02T00:00:00Z").unwrap().number,
            "2.0"
        );
        assert!(in_force(&all, "2025-01-01T00:00:00Z").is_none());
    }
}
