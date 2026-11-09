//! Documents through the real admin router: a draft previewed as its PDF, a
//! version published with its scheme's number and stored PDF, an unchanged
//! draft refused, and publishing kept from an operator who may only edit.

use std::sync::Arc;

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db, Module, Registry};
use rainmill_legal::{store, LegalModule};
use rainmill_pdf::PdfModule;
use tower::ServiceExt;

const BASE: &str = "/rainmill/legal";

async fn setup() -> (
    Db,
    laterite_core::testing::TestGuard,
    Router,
    std::path::PathBuf,
) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(rainmill_pdf::migrations::migrations());
    migrations.push(rainmill_legal::migrations::migrations());
    let (db, guard) = laterite_core::testing::connect_test(&migrations).await;
    let storage = std::env::temp_dir().join(format!(
        "rainmill-legal-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    (db.clone(), guard, app(&db, &storage), storage)
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

fn app(db: &Db, storage: &std::path::Path) -> Router {
    use laterite_admin::picker::PickerSourceReg;
    use laterite_admin::routes::ScreenReg;
    use laterite_admin::{Permission, Resource};
    let mut pdf = Registry::new();
    pdf.set_owner(PdfModule.id());
    PdfModule.register(&mut pdf);
    let mut registry = Registry::new();
    registry.set_owner(LegalModule.id());
    LegalModule.register(&mut registry);
    let mut resources = registry.take::<Resource>();
    for r in &mut resources {
        r.base_path = format!("{BASE}{}", r.base_path);
        if let Some(form) = r.form.as_mut() {
            form.base_path = r.base_path.clone();
        }
    }
    let mut screens = registry.take::<ScreenReg>();
    for s in &mut screens {
        s.base_path = format!("{BASE}{}", s.base_path);
    }
    let mut permissions = registry.take::<Permission>();
    permissions.extend(pdf.take::<Permission>());
    let picker_sources = pdf.take::<PickerSourceReg>();
    let listeners = registry.take::<laterite_core::ModelListenerReg>();
    let nav = registry.take::<laterite_admin::nav::NavItem>();
    let assets = registry.take::<laterite_admin::AssetReg>();
    let mut config = AdminConfig::default();
    config.storage_dir = storage.to_string_lossy().to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources,
            screens,
            permissions,
            picker_sources,
            listeners,
            nav,
            assets,
            plugin_defined: Arc::new(registry),
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// A session: a superuser with no grants listed, else an operator holding them.
async fn session(db: &Db, name: &str, grants: &[&str]) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    if grants.is_empty() {
        svc.create_superuser(NewOperator {
            username: name,
            email: &format!("{name}@acme.test"),
            first_name: name,
            last_name: None,
            password: "secretpw12345",
            timezone: None,
        })
        .await
        .unwrap();
    } else {
        let hash = laterite_auth::password::hash_password("secretpw12345").unwrap();
        let user = laterite_auth::store::create_user(
            db,
            name,
            &format!("{name}@acme.test"),
            name,
            None,
            &hash,
            false,
        )
        .await
        .unwrap();
        let grants: Vec<String> = grants.iter().map(|g| String::from(*g)).collect();
        let role = laterite_auth::store::create_role(db, name, name, &grants)
            .await
            .unwrap();
        laterite_auth::store::assign_role(db, user, role)
            .await
            .unwrap();
    }
    let token = svc
        .authenticate(name, "secretpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    token
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = laterite_admin::axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

fn get(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("cookie", format!("laterite_session={token}"))
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("laterite_session={token}"))
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn a_draft_is_previewed_published_numbered_and_stored() {
    let (db, _guard, app, storage) = setup().await;
    let root = session(&db, "root", &[]).await;

    let (status, _, _) = send(
        &app,
        post(
            &format!("/admin{BASE}/documents/new"),
            &root,
            "title=Privacy+Policy&slug=privacy&numbering=major_minor&body=We+collect+your+phone.",
        ),
    )
    .await;
    assert!(status.is_redirection() || status.is_success(), "{status}");
    let doc = store::document_by_slug(&db, "privacy")
        .await
        .unwrap()
        .expect("saved");
    assert_eq!(doc.body, "We collect your phone.");

    let (status, headers, body) = send(
        &app,
        post(
            &format!("/admin{BASE}/tools/preview"),
            &root,
            "title=Privacy+Policy&text=%23+Hello",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .iter()
        .any(|(k, v)| k == "content-type" && v == "application/pdf"));
    assert!(body.starts_with(b"%PDF"));

    let publish = format!("/admin{BASE}/tools/documents/{}/publish", doc.id);
    let (status, _, _) = send(
        &app,
        post(
            &publish,
            &root,
            "effective=2026-11-09&summary=First+version",
        ),
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    let versions = store::versions(&db, doc.id).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].number, "1.0");
    assert_eq!(
        versions[0].content_hash,
        store::content_hash("We collect your phone.")
    );
    assert!(
        versions[0].pdf_media_id.is_some(),
        "the PDF is stored at publish"
    );

    // Publishing the same text again is refused, naming the version it matches.
    let (status, _, html) = send(
        &app,
        post(&publish, &root, "effective=2026-11-10&summary=Again"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&html).contains("unchanged since version 1.0"));

    // A material change to the draft is 2.0.
    let (status, _, _) = send(
        &app,
        post(
            &format!("/admin{BASE}/documents/{}/edit", doc.id),
            &root,
            "title=Privacy+Policy&slug=privacy&numbering=major_minor&body=We+collect+your+phone+and+email.",
        ),
    )
    .await;
    assert!(status.is_redirection() || status.is_success(), "{status}");
    let (status, _, _) = send(
        &app,
        post(
            &publish,
            &root,
            "effective=2026-11-10&summary=We+now+collect+email&material=on",
        ),
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    let versions = store::versions(&db, doc.id).await.unwrap();
    assert_eq!(versions[0].number, "2.0");
    assert!(versions[0].material);

    let (status, _, html) = send(
        &app,
        get(
            &format!("/admin{BASE}/tools/documents/{}/versions", doc.id),
            &root,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = String::from_utf8_lossy(&html);
    assert!(
        html.contains("2.0") && html.contains("We now collect email") && html.contains("In force")
    );
    let _ = std::fs::remove_dir_all(storage);
}

#[tokio::test]
async fn an_editor_without_the_publish_grant_cannot_publish() {
    let (db, _guard, app, storage) = setup().await;
    let editor = session(&db, "editor", &[rainmill_legal::admin::EDIT]).await;
    let (status, _, _) = send(
        &app,
        post(
            &format!("/admin{BASE}/documents/new"),
            &editor,
            "title=Terms&slug=terms&numbering=date&body=Be+kind.",
        ),
    )
    .await;
    assert!(status.is_redirection() || status.is_success(), "{status}");
    let doc = store::document_by_slug(&db, "terms")
        .await
        .unwrap()
        .unwrap();
    let (_, _, html) = send(
        &app,
        get(&format!("/admin{BASE}/documents/{}/edit", doc.id), &editor),
    )
    .await;
    assert!(!String::from_utf8_lossy(&html).contains("Publish new version"));
    let (status, _, _) = send(
        &app,
        post(
            &format!("/admin{BASE}/tools/documents/{}/publish", doc.id),
            &editor,
            "effective=2026-11-09&summary=x",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(store::versions(&db, doc.id).await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(storage);
}

#[tokio::test]
async fn acceptances_record_the_version_in_force_and_changes_reach_those_behind() {
    use rainmill_legal::{accept, AcceptError, Method};
    let (db, _guard, app, storage) = setup().await;
    let root = session(&db, "root", &[]).await;
    send(
        &app,
        post(
            &format!("/admin{BASE}/documents/new"),
            &root,
            "title=Privacy+Policy&slug=privacy&numbering=major_minor&body=Version+one.",
        ),
    )
    .await;
    let doc = store::document_by_slug(&db, "privacy")
        .await
        .unwrap()
        .unwrap();

    let none = accept(&db, "privacy", ("launch_signup", "1"), Method::Form, None).await;
    assert!(
        matches!(none, Err(AcceptError::NothingInForce(_))),
        "nothing published yet"
    );

    let publish = format!("/admin{BASE}/tools/documents/{}/publish", doc.id);
    send(
        &app,
        post(
            &publish,
            &root,
            "effective=2026-01-01&summary=First+version",
        ),
    )
    .await;
    let v1 = accept(
        &db,
        "privacy",
        ("launch_signup", "1"),
        Method::Form,
        Some("ada@acme.test"),
    )
    .await
    .unwrap();
    assert_eq!(v1.number, "1.0");
    accept(&db, "privacy", ("launch_signup", "2"), Method::Form, None)
        .await
        .unwrap();

    send(
        &app,
        post(
            &format!("/admin{BASE}/documents/{}/edit", doc.id),
            &root,
            "title=Privacy+Policy&slug=privacy&numbering=major_minor&body=Version+two.",
        ),
    )
    .await;
    send(
        &app,
        post(
            &publish,
            &root,
            "effective=2026-02-01&summary=We+now+keep+your+email&material=on",
        ),
    )
    .await;
    // Someone who already accepted 2.0 is not behind.
    accept(
        &db,
        "privacy",
        ("launch_signup", "3"),
        Method::Click,
        Some("cy@acme.test"),
    )
    .await
    .unwrap();

    let notify = format!("/admin{BASE}/tools/documents/{}/notify", doc.id);
    let (status, _, html) = send(&app, get(&notify, &root)).await;
    assert_eq!(status, StatusCode::OK);
    let html = String::from_utf8_lossy(&html);
    assert!(
        html.contains("2 behind version 2.0: 1 with an email, 1 without."),
        "{html}"
    );
    assert!(
        html.contains("We now keep your email") && html.contains("/legal/privacy/compare/1.0/2.0")
    );

    let (status, _, _) = send(&app, post(&notify, &root, "to=2.0")).await;
    assert!(status.is_redirection(), "{status}");
    let (sql, values) = laterite_core::query::build(
        db.backend,
        sea_query::Query::select()
            .columns(["email", "status", "from_number", "to_number"].map(sea_query::Alias::new))
            .from(sea_query::Alias::new("legal_notices"))
            .to_owned(),
    );
    let rows = laterite_core::query::bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap();
    use laterite_core::AnyRowExt;
    assert_eq!(rows.len(), 1, "only the person behind with an email");
    assert_eq!(rows[0].get_text("email").unwrap(), "ada@acme.test");
    assert_eq!(rows[0].get_text("status").unwrap(), "queued");
    assert_eq!(rows[0].get_text("from_number").unwrap(), "1.0");

    let (status, _, html) = send(&app, get(&format!("/admin{BASE}/notices"), &root)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&html).contains("ada@acme.test"));
    let (status, _, _) = send(&app, get(&format!("/admin{BASE}/acceptances"), &root)).await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(storage);
}

/// Every save is kept; Undo steps back one save per click, recording itself;
/// the history shows each save's changes and Restore brings any back.
#[tokio::test]
async fn saves_are_kept_undone_one_at_a_time_and_restorable() {
    let (db, _guard, app, storage) = setup().await;
    let root = session(&db, "root", &[]).await;
    send(
        &app,
        post(
            &format!("/admin{BASE}/documents/new"),
            &root,
            "title=Terms&slug=terms&numbering=major_minor&body=One.",
        ),
    )
    .await;
    let doc = store::document_by_slug(&db, "terms")
        .await
        .unwrap()
        .unwrap();
    let edit = format!("/admin{BASE}/documents/{}/edit", doc.id);
    for body in ["Two.", "Three."] {
        let form = format!("title=Terms&slug=terms&numbering=major_minor&body={body}");
        send(&app, post(&edit, &root, &form)).await;
    }
    let body = || async { store::document(&db, doc.id).await.unwrap().unwrap().body };
    assert_eq!(body().await, "Three.");

    let undo = format!("/admin{BASE}/tools/documents/{}/undo", doc.id);
    let (status, _, _) = send(&app, post(&undo, &root, "")).await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(body().await, "Two.", "one save back");
    send(&app, post(&undo, &root, "")).await;
    assert_eq!(body().await, "One.", "and another");
    send(&app, post(&undo, &root, "")).await;
    assert_eq!(body().await, "One.", "nothing earlier");

    let (status, _, html) = send(
        &app,
        get(
            &format!("/admin{BASE}/tools/documents/{}/drafts", doc.id),
            &root,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = String::from_utf8_lossy(&html).to_string();
    assert!(
        html.contains("Undo") && html.contains("Current") && html.contains("<ins>"),
        "{html}"
    );

    let three = rainmill_legal::drafts::list(&db, doc.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.body == "Three.")
        .unwrap();
    let (status, _, _) = send(
        &app,
        post(
            &format!(
                "/admin{BASE}/tools/documents/{}/drafts/{}/restore",
                doc.id, three.id
            ),
            &root,
            "",
        ),
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(body().await, "Three.");
    assert!(
        store::versions(&db, doc.id).await.unwrap().is_empty(),
        "no save is a version"
    );
    let _ = std::fs::remove_dir_all(storage);
}
