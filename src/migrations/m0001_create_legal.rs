//! legal_documents and legal_versions: a document's draft, and the versions
//! published from it, each immutable once written.

use laterite_core::sea_query;
use laterite_core::strata::*;
use sea_query::Alias;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_legal"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        let id = || {
            ColumnDef::new(Alias::new("id"))
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key()
                .to_owned()
        };
        let text = |name: &str| ColumnDef::new(Alias::new(name)).text().null().to_owned();
        s.exec(
            Table::create()
                .table(Alias::new("legal_documents"))
                .col(id())
                // The document's address on the site (`privacy`) and its key in code.
                .col(key_col(Alias::new("slug")).not_null().unique_key())
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .col(text("description"))
                // major_minor, date or free: how the next version is numbered.
                .col(key_col(Alias::new("numbering")).not_null().default("major_minor"))
                // A PDF layout; the default layout when empty.
                .col(ColumnDef::new(Alias::new("layout_id")).big_integer().null())
                // The draft: markdown, never public until published.
                .col(text("body"))
                .col(text("draft_updated_by"))
                .col(text("created_at"))
                .col(text("updated_at"))
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("legal_versions"))
                .col(id())
                .col(
                    ColumnDef::new(Alias::new("document_id"))
                        .big_integer()
                        .not_null(),
                )
                // As the author chose it: 2.0, 2026-11-09, or free text.
                .col(key_col(Alias::new("number")).not_null())
                // When it takes force (UTC); may be in the future.
                .col(ColumnDef::new(Alias::new("effective_at")).text().not_null())
                .col(ColumnDef::new(Alias::new("published_at")).text().not_null())
                .col(text("published_by"))
                // What changed, in plain words.
                .col(ColumnDef::new(Alias::new("summary")).text().not_null())
                // A material change: the one people are told about.
                .col(bool_col(Alias::new("material")).not_null().default(0))
                .col(ColumnDef::new(Alias::new("body")).text().not_null())
                // BLAKE3 of the body, hex: what was published, provably.
                .col(ColumnDef::new(Alias::new("content_hash")).text().not_null())
                // The PDF rendered at publish, a media record.
                .col(ColumnDef::new(Alias::new("pdf_media_id")).big_integer().null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("legal_versions_number")
                .table(Alias::new("legal_versions"))
                .col(Alias::new("document_id"))
                .col(Alias::new("number"))
                .unique()
                .to_owned(),
        )
        .await
    }
}
