//! What happens when the admin saves a document: who saved the draft is kept.

use laterite_core::strata::async_trait;
use laterite_core::{ErrorBag, ModelListener, Op, Record, SaveCx};

/// Records who last saved a document's draft.
pub(crate) struct DraftAuthor;

#[async_trait]
impl ModelListener for DraftAuthor {
    async fn before_save(
        &self,
        cx: &mut SaveCx<'_>,
        rec: &mut Record,
        _op: Op,
    ) -> Result<(), ErrorBag> {
        rec.set("draft_updated_by", cx.actor().label().to_string());
        Ok(())
    }
}
