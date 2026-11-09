//! What happens when the admin saves a document: who saved the draft is kept,
//! and its state joins the draft history.

use laterite_core::query::bind_values;
use laterite_core::strata::async_trait;
use laterite_core::{ErrorBag, ModelListener, Op, Record, SaveCx, SavedCx, Text};

use crate::drafts::{self, Kind};

/// Records who last saved a document's draft.
pub(crate) struct DraftAuthor;

#[async_trait]
impl ModelListener for DraftAuthor {
    async fn before_save(
        &self,
        cx: &mut SaveCx<'_>,
        rec: &mut Record,
        op: Op,
    ) -> Result<(), ErrorBag> {
        rec.set("draft_updated_by", cx.actor().label().to_string());
        if let (Op::Update, Some(id)) = (op, rec.id()) {
            let author = cx.actor().label().to_string();
            let backend = cx.backend();
            // The state before history began; never blocks the save.
            let _ =
                drafts::record_on(cx.conn(), backend, id, Kind::Save, None, &author, true).await;
        }
        Ok(())
    }

    async fn after_save(&self, cx: &SavedCx<'_>, rec: &Record, _op: Op) {
        if let Some(id) = rec.id() {
            let _ = drafts::record(cx.db(), id, Kind::Save, None, cx.actor().label()).await;
        }
    }

    async fn before_delete(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<(), Text> {
        let Some(id) = rec.id() else { return Ok(()) };
        let (sql, values) = drafts::drop_sql(cx.backend(), id);
        bind_values(sqlx::query(&sql), values)
            .execute(&mut *cx.conn())
            .await
            .map_err(|e| Text::from(e.to_string()))?;
        Ok(())
    }
}
