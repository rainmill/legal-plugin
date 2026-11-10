//! The site component: the version in force, a named version, the history,
//! and what changed between two versions or two dates, through its default
//! markup.

use laterite_core::minijinja;
use laterite_core::sea_query;
use laterite_core::serde_json;
use laterite_core::sqlx;
use std::collections::BTreeMap;

use laterite_core::{Module, Registry};
use laterite_web::theme::component::{ComponentContext, ComponentReg};
use rainmill_legal::{store, LegalModule};
use serde_json::{json, Value};

async fn setup() -> (
    laterite_core::Db,
    laterite_core::testing::TestGuard,
    ComponentReg,
) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(rainmill_pdf::migrations::migrations());
    migrations.push(rainmill_legal::migrations::migrations());
    let (db, guard) = laterite_core::testing::connect_test(&migrations).await;
    let mut registry = Registry::new();
    registry.set_owner(LegalModule.id());
    LegalModule.register(&mut registry);
    let reg = registry
        .items::<ComponentReg>()
        .into_iter()
        .find(|c| c.code == "rainmill.legal.document")
        .expect("the plugin registers its component")
        .clone();
    (db, guard, reg)
}

async fn set_draft(db: &laterite_core::Db, slug: &str, body: &str) -> i64 {
    use sea_query::{Alias, Expr, Query};
    let (sql, values) = match store::document_by_slug(db, slug).await.unwrap() {
        Some(doc) => laterite_core::query::build(
            db.backend,
            Query::update()
                .table(Alias::new("legal_documents"))
                .value(Alias::new("body"), body)
                .and_where(Expr::col(Alias::new("id")).eq(doc.id))
                .to_owned(),
        ),
        None => laterite_core::query::build(
            db.backend,
            Query::insert()
                .into_table(Alias::new("legal_documents"))
                .columns(["slug", "title", "numbering", "body"].map(Alias::new))
                .values_panic([
                    slug.into(),
                    "Privacy Policy".into(),
                    "major_minor".into(),
                    body.into(),
                ])
                .to_owned(),
        ),
    };
    laterite_core::query::bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await
        .unwrap();
    store::document_by_slug(db, slug).await.unwrap().unwrap().id
}

async fn run(db: &laterite_core::Db, reg: &ComponentReg, params: &[(&str, &str)]) -> Value {
    let properties = json!({ "slug": "privacy", "url": "/legal/{slug}" })
        .as_object()
        .unwrap()
        .clone();
    let params: BTreeMap<String, String> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let cx = ComponentContext {
        db,
        properties: &properties,
        params: &params,
        path: "/legal/privacy",
        theme: "demo",
        preview: false,
    };
    reg.component.run(&cx).await.unwrap().data
}

fn render(reg: &ComponentReg, data: &Value) -> String {
    let env = minijinja::Environment::new();
    let context = minijinja::Value::from(minijinja::value::Serde(json!({ "component": data })));
    env.render_str(reg.component.markup().unwrap(), context)
        .unwrap()
}

#[tokio::test]
async fn the_page_shows_the_version_in_force_its_history_and_the_changes() {
    let (db, _guard, reg) = setup().await;
    let id = set_draft(&db, "privacy", "We collect your phone.").await;
    let publish = |number: &str, effective: &str, summary: &str| store::Publish {
        number: number.into(),
        effective: effective.into(),
        summary: summary.into(),
        material: false,
    };
    store::publish(&db, id, &publish("", "2026-01-01", "First version"), "ada")
        .await
        .unwrap();
    set_draft(&db, "privacy", "We collect your phone and email.").await;
    store::publish(&db, id, &publish("", "2026-06-01", "Adds email"), "ada")
        .await
        .unwrap();
    set_draft(&db, "privacy", "We collect your phone, email and handle.").await;
    store::publish(
        &db,
        id,
        &publish("", "2999-01-01", "Adds the handle"),
        "ada",
    )
    .await
    .unwrap();

    let current = run(&db, &reg, &[]).await;
    assert_eq!(
        current["version"]["number"], "1.1",
        "the latest already in force"
    );
    assert!(current["version"]["html"]
        .as_str()
        .unwrap()
        .contains("phone and email"));
    assert_eq!(current["history"].as_array().unwrap().len(), 3);
    assert_eq!(
        current["history"][0]["scheduled"], true,
        "1.2 takes force later"
    );
    let html = render(&reg, &current);
    assert!(
        html.contains("Version 1.1")
            && html.contains("(in force)")
            && html.contains("/legal/privacy/1.0")
    );

    let old = run(&db, &reg, &[("version", "1.0")]).await;
    assert!(old["version"]["html"]
        .as_str()
        .unwrap()
        .contains("your phone."));

    let by_version = run(&db, &reg, &[("from", "1.0"), ("to", "1.2")]).await;
    let changes = by_version["compare"]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2, "1.1 and 1.2");
    assert!(by_version["compare"]["html"]
        .as_str()
        .unwrap()
        .contains("<ins>"));

    let by_date = run(&db, &reg, &[("from", "2026-02-01"), ("to", "2026-07-01")]).await;
    assert_eq!(by_date["compare"]["from"]["number"], "1.0");
    assert_eq!(by_date["compare"]["to"]["number"], "1.1");
    assert!(render(&reg, &by_date).contains("What changed from version 1.0 to 1.1"));

    let missing = run(&db, &reg, &[("version", "9.9")]).await;
    assert_eq!(missing["missing"], "9.9");
    assert!(render(&reg, &missing).contains("There is no version 9.9"));
}

#[tokio::test]
async fn a_version_lists_its_sections() {
    let (db, _guard, reg) = setup().await;
    let id = set_draft(
        &db,
        "privacy",
        "## Who we are\n\nAcme.\n\n## Your *rights*\n\nAsk.",
    )
    .await;
    let input = store::Publish {
        number: String::new(),
        effective: "2026-01-01".into(),
        summary: "First".into(),
        material: false,
    };
    store::publish(&db, id, &input, "ada").await.unwrap();
    let data = run(&db, &reg, &[]).await;
    let toc = data["version"]["toc"].as_array().unwrap();
    assert_eq!(toc.len(), 2);
    assert_eq!(toc[0]["id"], "who-we-are");
    assert_eq!(toc[1]["title"], "Your rights");
}
