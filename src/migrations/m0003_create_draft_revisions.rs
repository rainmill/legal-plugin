//! legal_draft_revisions: a document's draft after each save, private to the
//! admin, for its draft history, Restore and Undo.

use laterite_core::sea_query;
use laterite_core::strata::*;
use sea_query::Alias;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0003_create_draft_revisions"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("legal_draft_revisions"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("document_id")).big_integer().not_null())
                .col(ColumnDef::new(Alias::new("title")).text().null())
                .col(ColumnDef::new(Alias::new("description")).text().null())
                .col(ColumnDef::new(Alias::new("layout_id")).big_integer().null())
                .col(ColumnDef::new(Alias::new("body")).text().null())
                // save, undo or restore: how this state came about.
                .col(key_col(Alias::new("kind")).not_null().default("save"))
                // For an undo or restore, the revision whose state it brought back.
                .col(ColumnDef::new(Alias::new("source_id")).big_integer().null())
                .col(ColumnDef::new(Alias::new("author")).text().null())
                .col(ColumnDef::new(Alias::new("created_at")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("legal_draft_revisions_document")
                .table(Alias::new("legal_draft_revisions"))
                .col(Alias::new("document_id"))
                .to_owned(),
        )
        .await
    }
}
