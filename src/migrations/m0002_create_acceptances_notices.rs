//! legal_acceptances and legal_notices: who accepted which version, and each
//! message telling someone what changed.

use laterite_core::sea_query;
use laterite_core::strata::*;
use sea_query::Alias;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0002_create_acceptances_notices"
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
        let big = |name: &str| ColumnDef::new(Alias::new(name)).big_integer().not_null().to_owned();
        s.exec(
            Table::create()
                .table(Alias::new("legal_acceptances"))
                .col(id())
                .col(big("document_id"))
                .col(big("version_id"))
                // Copied in: a version never changes, and the list reads without a join.
                .col(key_col(Alias::new("document_slug")).not_null())
                .col(key_col(Alias::new("version_number")).not_null())
                // Who accepted: an entity and its id, as media links name an owner.
                .col(key_col(Alias::new("subject_type")).not_null())
                .col(key_col(Alias::new("subject_id")).not_null())
                // Where to tell them about a change, when known.
                .col(text("email"))
                // form, click or import.
                .col(key_col(Alias::new("method")).not_null().default("form"))
                .col(ColumnDef::new(Alias::new("accepted_at")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("legal_acceptances_subject")
                .table(Alias::new("legal_acceptances"))
                .col(Alias::new("subject_type"))
                .col(Alias::new("subject_id"))
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("legal_notices"))
                .col(id())
                .col(big("document_id"))
                .col(big("to_version_id"))
                .col(key_col(Alias::new("document_slug")).not_null())
                .col(key_col(Alias::new("from_number")).not_null())
                .col(key_col(Alias::new("to_number")).not_null())
                .col(key_col(Alias::new("subject_type")).not_null())
                .col(key_col(Alias::new("subject_id")).not_null())
                .col(ColumnDef::new(Alias::new("email")).text().not_null())
                // mail, the one channel so far.
                .col(key_col(Alias::new("channel")).not_null().default("mail"))
                // queued (handed to the mail queue) or failed (refused before it).
                .col(key_col(Alias::new("status")).not_null().default("queued"))
                .col(ColumnDef::new(Alias::new("attempts")).integer().not_null().default(0))
                .col(text("last_error"))
                .col(text("created_at"))
                .col(text("updated_at"))
                .to_owned(),
        )
        .await
    }
}
