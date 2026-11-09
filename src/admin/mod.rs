//! The admin: documents as a descriptor, and the route behind the editor's
//! live PDF of the draft.

use std::collections::HashMap;
use std::sync::Arc;

use laterite_admin::axum::extract::{Form, Path};
use laterite_admin::axum::http::{header, HeaderMap, StatusCode};
use laterite_admin::axum::response::{Html, IntoResponse, Response};
use laterite_admin::axum::routing::{get, post};
use laterite_admin::axum::{Extension, Router};
use laterite_admin::html::Markup;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{
    AuthenticatedUser, FlashLevel, PageHead, Permission, Resource, SessionHandle, Shell,
};
use laterite_core::t;
use rainmill_pdf::history::{escape, word_diff};
use rainmill_pdf::{DocumentMeta, Output, Pdf, RenderOptions};

use crate::store::{self, Document, Publish, Version};

/// Editing drafts: opening, creating and saving documents.
pub const EDIT: &str = "rainmill.legal.edit";
/// Publishing a version, a legal act kept apart from editing.
pub const PUBLISH: &str = "rainmill.legal.publish";
/// Sending the changes between versions to the people they affect.
pub const NOTIFY: &str = "rainmill.legal.notify";
/// The menu section holding the Legal screens.
pub const SECTION: &str = "rainmill.legal";

pub(crate) fn permissions() -> Vec<Permission> {
    let group = || t!("Legal");
    vec![
        Permission::new(EDIT, t!("Edit legal document drafts"), group()),
        Permission::new(PUBLISH, t!("Publish legal document versions"), group()),
        Permission::new(NOTIFY, t!("Send legal document changes"), group()),
    ]
}

pub(crate) fn section() -> laterite_admin::nav::NavItem {
    laterite_admin::nav::NavItem::section(SECTION, t!("Legal")).icon("scale")
}

pub(crate) fn resources() -> Vec<Resource> {
    vec![laterite_admin::resource!("admin/documents.yaml")]
}

pub(crate) fn screen() -> ScreenReg {
    ScreenReg::new("/tools", EDIT, Arc::new(Tools))
}

struct Tools;

#[derive(Clone)]
struct Cx {
    ctx: RouteCtx,
    pdf: Pdf,
}

impl Cx {
    fn edit(&self, id: i64) -> String {
        format!("{}/rainmill/legal/documents/{id}/edit", self.admin())
    }

    fn tools(&self) -> String {
        format!("{}/rainmill/legal/tools", self.admin())
    }

    fn admin(&self) -> String {
        self.ctx.admin_path().trim_end_matches('/').to_string()
    }
}

/// Lets a request through only with `code`.
fn guarded(
    code: &'static str,
) -> impl Fn(
    Extension<Shell>,
    laterite_admin::axum::extract::Request,
    laterite_admin::axum::middleware::Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send
       + Sync
       + 'static {
    move |Extension(shell): Extension<Shell>, req, next: laterite_admin::axum::middleware::Next| {
        Box::pin(async move {
            if shell.allows(code) {
                next.run(req).await
            } else {
                StatusCode::FORBIDDEN.into_response()
            }
        })
    }
}

impl Screen for Tools {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let library =
            laterite_admin::medialib::library_driver(ctx.plugin_defined(), ctx.storage_dir());
        let pdf = Pdf::new(ctx.db().clone(), ctx.plugin_defined())
            .disk(laterite_admin::medialib::DISK, library);
        let cx = Cx {
            ctx: ctx.clone(),
            pdf,
        };
        let (preview_pdf, versions_cx, page_cx, publish_cx, render_cx) = (
            cx.pdf.clone(),
            cx.clone(),
            cx.clone(),
            cx.clone(),
            cx.clone(),
        );
        let publishing = Router::new()
            .route(
                "/documents/{id}/publish",
                get(
                    move |Path(id): Path<i64>, Extension(shell): Extension<Shell>| {
                        let cx = page_cx.clone();
                        async move { publish_page(&cx, id, shell, None, None).await }
                    },
                )
                .post(
                    move |Path(id): Path<i64>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>,
                          headers: HeaderMap,
                          Form(form): Form<HashMap<String, String>>| {
                        let cx = publish_cx.clone();
                        async move { publish(&cx, id, shell, user, session, headers, form).await }
                    },
                ),
            )
            .route(
                "/versions/{id}/pdf",
                post(
                    move |Path(id): Path<i64>,
                          Extension(session): Extension<SessionHandle>,
                          headers: HeaderMap| {
                        let cx = render_cx.clone();
                        async move { rerender(&cx, id, session, headers).await }
                    },
                ),
            )
            .route_layer(laterite_admin::axum::middleware::from_fn(guarded(PUBLISH)));
        Router::new()
            .route(
                "/preview",
                post(move |Form(form): Form<HashMap<String, String>>| {
                    let pdf = preview_pdf.clone();
                    async move { preview(&pdf, form).await }
                }),
            )
            .route(
                "/documents/{id}/versions",
                get(
                    move |Path(id): Path<i64>, Extension(shell): Extension<Shell>| {
                        let cx = versions_cx.clone();
                        async move { versions_page(&cx, id, shell).await }
                    },
                ),
            )
            .merge(publishing)
    }
}

fn redirect(to: String, headers: &HeaderMap) -> Response {
    if headers.contains_key("hx-request") {
        ([("HX-Redirect", to)], StatusCode::NO_CONTENT).into_response()
    } else {
        (StatusCode::SEE_OTHER, [(header::LOCATION, to)]).into_response()
    }
}

fn failed(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}

/// The publish dialog: the changes since the latest version, then the number,
/// the effective date, whether the change is material, and what changed.
async fn publish_page(
    cx: &Cx,
    id: i64,
    shell: Shell,
    error: Option<String>,
    form: Option<&HashMap<String, String>>,
) -> Response {
    let doc = match store::document(cx.ctx.db(), id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failed(e),
    };
    let versions = match store::versions(cx.ctx.db(), id).await {
        Ok(v) => v,
        Err(e) => return failed(e),
    };
    let today = chrono::Utc::now().date_naive().to_string();
    let value = |k: &str, default: &str| {
        form.and_then(|f| f.get(k))
            .map(String::as_str)
            .unwrap_or(default)
            .to_string()
    };
    let minor = store::next_number(doc.numbering, &versions, false, &today);
    let major = store::next_number(doc.numbering, &versions, true, &today);
    let mut body = String::new();
    if let Some(error) = error {
        body.push_str(&format!(
            "<div class=\"lat-alert\">{}</div>",
            escape(&error)
        ));
    }
    body.push_str("<div class=\"lat-card\" style=\"padding:18px 20px;margin-bottom:16px\">");
    match versions.first() {
        Some(latest) if latest.body == doc.body => body.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&format!(
                "The draft is the text of version {}: change it before publishing.",
                latest.number
            ))
        )),
        Some(latest) => body.push_str(&format!(
            "<h3 class=\"lat-field__heading\">{}</h3><div class=\"rm-legal-diff\">{}</div>",
            escape(&format!("Changes since version {}", latest.number)),
            word_diff(&latest.body, &doc.body)
        )),
        None => body.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&shell.t("This will be the first version."))
        )),
    }
    body.push_str("</div>");
    let numbering_help = match doc.numbering {
        store::Numbering::MajorMinor if versions.is_empty() => format!("Leave empty for {minor}."),
        store::Numbering::MajorMinor => {
            format!("Leave empty for {major} if the change is material, {minor} if not.")
        }
        store::Numbering::Date => format!("Leave empty for {minor}, from the effective date."),
        store::Numbering::Free => "Any number or name not used before.".to_string(),
    };
    body.push_str(&format!(
        r#"<div class="lat-card lat-card--form"><form method="post" action="{action}"><input type="hidden" name="_csrf" value="{csrf}"><div class="lat-fields">
<div class="lat-field lat-field--3"><label class="lat-field__label" for="rm-number">{l_number}</label><input class="lat-input" id="rm-number" name="number" value="{number}" placeholder="{placeholder}"><p class="lat-field__help">{h_number}</p></div>
<div class="lat-field lat-field--3"><label class="lat-field__label" for="rm-effective">{l_effective}</label><input class="lat-input" type="date" id="rm-effective" name="effective" value="{effective}" required><p class="lat-field__help">{h_effective}</p></div>
<div class="lat-field lat-field--6"><label class="lat-field__label" for="rm-material">{l_material}</label><label class="lat-check lat-check--control"><input class="lat-switch" type="checkbox" role="switch" id="rm-material" name="material" value="on"{checked}></label><p class="lat-field__help">{h_material}</p></div>
<div class="lat-field"><label class="lat-field__label" for="rm-summary">{l_summary}<span class="lat-field__req">*</span></label><textarea class="lat-input" id="rm-summary" name="summary" rows="4" style="--rows:4" required>{summary}</textarea><p class="lat-field__help">{h_summary}</p></div>
</div><div class="lat-actions"><button class="lat-btn lat-btn--primary" type="submit" data-lat-confirm="{confirm}">{l_publish}</button><a class="lat-btn lat-btn--ghost" href="{cancel}">{l_cancel}</a></div></form></div>"#,
        action = escape(&format!("{}/documents/{id}/publish", cx.tools())),
        csrf = escape(shell.csrf_token()),
        l_number = escape(&shell.t("Version number")),
        number = escape(&value("number", "")),
        placeholder = escape(&minor),
        h_number = escape(&numbering_help),
        l_effective = escape(&shell.t("Takes effect")),
        effective = escape(&value("effective", &today)),
        h_effective = escape(&shell.t("May be in the future; the current version stays in force until then.")),
        l_material = escape(&shell.t("Material change")),
        checked = if value("material", "") == "on" { " checked" } else { "" },
        h_material = escape(&shell.t("A change people need to know about. Only material changes are sent to them.")),
        l_summary = escape(&shell.t("What changed")),
        summary = escape(&value("summary", "")),
        h_summary = escape(&shell.t("In plain words, shown with the version and in any message about it.")),
        confirm = escape(&shell.t("Publish this version? It cannot be changed once published.")),
        l_publish = escape(&shell.t("Publish")),
        cancel = escape(&cx.edit(id)),
        l_cancel = escape(&shell.t("Cancel")),
    ));
    shell.page_with(
        PageHead::new(laterite_core::Text::dynamic(format!(
            "{}: {}",
            shell.t("Publish"),
            doc.title
        )))
        .back(t!("Back"), cx.edit(id))
        .assets(&[DIFF_CSS]),
        Markup::raw_unchecked(body),
    )
}

/// Publishes the draft, stores its PDF and writes the audit entry.
#[allow(clippy::too_many_arguments)]
async fn publish(
    cx: &Cx,
    id: i64,
    shell: Shell,
    user: AuthenticatedUser,
    session: SessionHandle,
    headers: HeaderMap,
    form: HashMap<String, String>,
) -> Response {
    let field = |k: &str| form.get(k).cloned().unwrap_or_default();
    let input = Publish {
        number: field("number"),
        effective: field("effective"),
        summary: field("summary"),
        material: matches!(field("material").as_str(), "on" | "1" | "true"),
    };
    let version = match store::publish(cx.ctx.db(), id, &input, shell.username()).await {
        Ok(v) => v,
        Err(store::PublishError::Db(e)) => return failed(e),
        Err(e) => return publish_page(cx, id, shell, Some(e.to_string()), Some(&form)).await,
    };
    let doc = store::document(cx.ctx.db(), id).await.ok().flatten();
    let title = doc.as_ref().map(|d| d.title.clone()).unwrap_or_default();
    cx.ctx
        .audit(
            &user,
            "rainmill.legal.version.publish",
            Some(crate::VERSIONS),
            Some(&version.id.to_string()),
            Some(&format!("{title} {}", version.number)),
            Some(&version.summary),
        )
        .await;
    match store_pdf(cx, doc.as_ref(), &version).await {
        Ok(()) => session.push_flash(
            FlashLevel::Success,
            laterite_core::Text::dynamic(format!("Published version {}.", version.number)),
        ),
        Err(e) => session.push_flash(
            FlashLevel::Error,
            laterite_core::Text::dynamic(format!(
                "Published version {}, but its PDF could not be stored ({e}). Render it again from Versions.",
                version.number
            )),
        ),
    }
    redirect(format!("{}/documents/{id}/versions", cx.tools()), &headers)
}

/// Renders `version` as an archival PDF in its document's layout and stores it.
async fn store_pdf(cx: &Cx, doc: Option<&Document>, version: &Version) -> Result<(), String> {
    let doc = doc.ok_or("the document is gone")?;
    let meta = DocumentMeta {
        title: doc.title.clone(),
        version: version.number.clone(),
        effective: store::long_date(&version.effective_at),
        description: doc.description.clone(),
        ..DocumentMeta::default()
    };
    let options = RenderOptions {
        output: Output::archival(),
        ..RenderOptions::default()
    };
    let mut rendered = cx
        .pdf
        .render_document(&version.body, doc.layout_id, meta, &options)
        .await
        .map_err(|e| e.to_string())?;
    rendered.filename = format!("{}-{}.pdf", doc.slug, version.number);
    let record = rendered
        .store(
            &cx.pdf,
            laterite_admin::medialib::DISK,
            Some(laterite_media::Owner {
                owner_type: crate::VERSIONS,
                owner_id: &version.id.to_string(),
                field: "pdf",
            }),
        )
        .await
        .map_err(|e| e.to_string())?;
    store::set_pdf(cx.ctx.db(), version.id, record.id)
        .await
        .map_err(|e| e.to_string())
}

/// Renders and stores a version's PDF again: after a failed store, or to set
/// it in a changed layout. The text never changes.
async fn rerender(
    cx: &Cx,
    version_id: i64,
    session: SessionHandle,
    headers: HeaderMap,
) -> Response {
    let Some(version) = find_version(cx, version_id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let doc = store::document(cx.ctx.db(), version.document_id)
        .await
        .ok()
        .flatten();
    match store_pdf(cx, doc.as_ref(), &version).await {
        Ok(()) => session.push_flash(FlashLevel::Success, t!("The PDF was rendered and stored.")),
        Err(e) => session.push_flash(
            FlashLevel::Error,
            laterite_core::Text::dynamic(format!("The PDF could not be stored: {e}")),
        ),
    }
    redirect(
        format!("{}/documents/{}/versions", cx.tools(), version.document_id),
        &headers,
    )
}

async fn find_version(cx: &Cx, id: i64) -> Option<Version> {
    let (sql, values) = laterite_core::query::build(
        cx.ctx.db().backend,
        sea_query::Query::select()
            .column(sea_query::Alias::new("document_id"))
            .from(sea_query::Alias::new(crate::VERSIONS))
            .and_where(sea_query::Expr::col(sea_query::Alias::new("id")).eq(id))
            .to_owned(),
    );
    let row = laterite_core::query::bind_values(sqlx::query(&sql), values)
        .fetch_optional(&cx.ctx.db().pool)
        .await
        .ok()??;
    use laterite_core::AnyRowExt;
    let document_id = row.get_int("document_id").ok()?;
    store::versions(cx.ctx.db(), document_id)
        .await
        .ok()?
        .into_iter()
        .find(|v| v.id == id)
}

/// Every version of a document, newest first: number, effective date, who
/// published it, what changed, which is in force, and its PDF.
async fn versions_page(cx: &Cx, id: i64, shell: Shell) -> Response {
    let doc = match store::document(cx.ctx.db(), id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failed(e),
    };
    let versions = match store::versions(cx.ctx.db(), id).await {
        Ok(v) => v,
        Err(e) => return failed(e),
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let current = store::in_force(&versions, &now).map(|v| v.id);
    let mut rows = String::new();
    for v in &versions {
        let pdf = match v.pdf_media_id {
            Some(media) => match laterite_media::record::find(cx.ctx.db(), media).await {
                Ok(Some(rec)) => format!(
                    r#"<a href="{}" target="_blank" rel="noopener">{}</a>"#,
                    escape(&laterite_admin::medialib::file_url(&rec)),
                    escape(&shell.t("PDF"))
                ),
                _ => String::new(),
            },
            None if shell.allows(PUBLISH) => laterite_admin::ui::Button::post(
                t!("Render PDF"),
                format!("{}/versions/{}/pdf", cx.tools(), v.id),
            )
            .small()
            .render(&shell)
            .into_string(),
            None => escape(&shell.t("Missing")),
        };
        let mut badges = String::new();
        if Some(v.id) == current {
            badges.push_str(
                &laterite_admin::ui::Badge::new(t!("In force"))
                    .render(&shell)
                    .into_string(),
            );
        } else if v.effective_at > now {
            badges.push_str(
                &laterite_admin::ui::Badge::new(t!("Scheduled"))
                    .render(&shell)
                    .into_string(),
            );
        }
        if v.material {
            badges.push(' ');
            badges.push_str(
                &laterite_admin::ui::Badge::new(t!("Material"))
                    .render(&shell)
                    .into_string(),
            );
        }
        rows.push_str(&format!(
            "<tr><td><strong>{}</strong> {badges}</td><td>{}</td><td>{}<br><span class=\"lat-muted\">{}</span></td><td>{}</td><td>{pdf}</td></tr>",
            escape(&v.number),
            escape(&store::long_date(&v.effective_at)),
            escape(&shell.datetime(&v.published_at)),
            escape(&v.published_by),
            escape(&v.summary),
        ));
    }
    let table = if versions.is_empty() {
        format!("<p class=\"lat-note\">{}</p>", escape(&shell.t("Nothing published yet. The draft stays private until its first version is published.")))
    } else {
        format!(
            "<div class=\"lat-tablewrap\"><table class=\"lat-table\"><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th></th></tr></thead><tbody>{rows}</tbody></table></div>",
            escape(&shell.t("Version")),
            escape(&shell.t("Takes effect")),
            escape(&shell.t("Published")),
            escape(&shell.t("What changed")),
        )
    };
    let mut head = PageHead::new(laterite_core::Text::dynamic(format!(
        "{}: {}",
        shell.t("Versions"),
        doc.title
    )))
    .back(t!("Back"), cx.edit(id));
    if shell.allows(PUBLISH) {
        head = head.actions(
            laterite_admin::ui::Button::link(
                t!("Publish new version"),
                format!("{}/documents/{id}/publish", cx.tools()),
            )
            .primary()
            .render(&shell),
        );
    }
    shell.page_with(head, Markup::raw_unchecked(table))
}

/// The diff stylesheet, by its asset key.
pub const DIFF_CSS: &str = "rainmill-legal-diff.css";

pub(crate) fn diff_css() -> laterite_admin::AdminAsset {
    laterite_admin::AdminAsset::new("text/css", include_bytes!("../../assets/diff.css"))
}

/// The draft being edited as its PDF, set in the form's layout, unsaved.
async fn preview(pdf: &Pdf, form: HashMap<String, String>) -> Response {
    let field = |k: &str| form.get(k).map(String::as_str).unwrap_or("");
    let meta = DocumentMeta {
        title: field("title").to_string(),
        version: "Draft".into(),
        description: field("description").to_string(),
        ..DocumentMeta::default()
    };
    match pdf
        .render_document(
            field("text"),
            field("layout_id").trim().parse().ok(),
            meta,
            &RenderOptions::default(),
        )
        .await
    {
        Ok(rendered) => rendered.inline(),
        Err(e) => pane_message(&e.to_string()),
    }
}

/// A message in the preview pane, in place of the document.
fn pane_message(message: &str) -> Response {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Html(format!("<p class=\"lat-markdown__empty\">{escaped}</p>")).into_response()
}
