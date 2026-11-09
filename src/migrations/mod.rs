//! This plugin's migrations, in apply order.

laterite_core::migration_set! {
    module_id: "rainmill.legal",
    m0001_create_legal,
    m0002_create_acceptances_notices,
    m0003_create_draft_revisions,
}
