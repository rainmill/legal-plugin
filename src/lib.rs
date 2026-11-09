//! Rainmill Legal: terms, privacy policies and any other legal document, with a
//! draft that is never public, versions published with a number the author
//! chooses, a public history with differences between versions, and a record of
//! who accepted which version.
//!
//! A document's PDF is set in a layout from the PDF plugin (`rainmill.pdf`),
//! which this module requires.
#![warn(missing_docs)]

pub mod admin;
mod listener;
pub mod migrations;
pub mod store;

use std::sync::Arc;

use laterite_core::strata::async_trait;
use laterite_core::{MigrationSet, ModelListenerReg, Module, ModuleId, Registry};

/// The documents table.
pub const DOCUMENTS: &str = "legal_documents";
/// The versions table.
pub const VERSIONS: &str = "legal_versions";

/// This plugin's entry point, collected by the generated plugins manifest.
pub fn module() -> Box<dyn Module> {
    Box::new(LegalModule)
}

/// The Rainmill Legal plugin.
pub struct LegalModule;

const REQUIRES: &[ModuleId] = &[ModuleId::new("rainmill.pdf")];

#[async_trait]
impl Module for LegalModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }

    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }

    fn requires(&self) -> &'static [ModuleId] {
        REQUIRES
    }

    fn register(&self, registry: &mut Registry) {
        use laterite_admin::AdminRegistry;
        for permission in admin::permissions() {
            registry.add_permission(permission);
        }
        registry.add_nav(admin::section());
        for resource in admin::resources() {
            registry.add_resource(resource);
        }
        registry.add_screen(admin::screen());
        registry.add_asset(admin::DIFF_CSS, admin::diff_css());
        registry.add(ModelListenerReg::for_entity(
            DOCUMENTS,
            Arc::new(listener::DraftAuthor),
        ));
    }
}
