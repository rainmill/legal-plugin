//! Sending the changes: the people whose latest acceptance is older than a
//! version are told what changed, by mail, with a link to the public compare
//! page.
//!
//! Each message is handed to the framework's mail queue, which retries it, logs
//! it and lists a final failure under Jobs with a Retry. A notice records
//! queued, or failed when the queue refused it, never "delivered": the mail log
//! is where delivery shows. Anyone with no email on record is counted on the
//! page, to be reached another way; the compare page is public for that.

use laterite_core::sqlx;
use std::collections::HashMap;

use laterite_admin::axum::http::StatusCode;
use laterite_admin::axum::response::{IntoResponse, Response};
use laterite_admin::html::Markup;
use laterite_admin::routes::RouteCtx;
use laterite_admin::{AuthenticatedUser, FlashLevel, PageHead, SessionHandle, Shell};
use laterite_core::mail::Message;
use laterite_core::t;
use rainmill_pdf::history::escape;

use crate::store::{self, Accepted, Document, Version};

/// The compare page address a message links to, by default.
const COMPARE: &str = "{base}/legal/{slug}/compare/{from}/{to}";

fn failed(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}

/// A message for someone who accepted `from` about everything up to `to`.
pub(crate) struct Composed {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// The message telling a person who accepted `from` what changed up to `to`.
pub(crate) fn compose(
    doc: &Document,
    versions: &[Version],
    from: &str,
    to: &Version,
    url: &str,
) -> Composed {
    let from_order = versions
        .iter()
        .find(|v| v.number == from)
        .map(|v| (v.published_at.clone(), v.id));
    let mut between: Vec<&Version> = versions
        .iter()
        .filter(|v| {
            let o = (v.published_at.clone(), v.id);
            from_order.as_ref().is_none_or(|f| &o > f) && o <= (to.published_at.clone(), to.id)
        })
        .collect();
    between.sort_by(|a, b| (a.published_at.as_str(), a.id).cmp(&(b.published_at.as_str(), b.id)));
    let effective = store::long_date(&to.effective_at);
    let subject = format!("{}: what changed in version {}", doc.title, to.number);
    let mut text = format!(
        "We have updated our {}. Version {} takes effect on {effective}.\n\nWhat changed since version {from}:\n",
        doc.title, to.number
    );
    let mut items = String::new();
    for v in &between {
        text.push_str(&format!(
            "- {} (effective {}): {}\n",
            v.number,
            store::long_date(&v.effective_at),
            v.summary
        ));
        items.push_str(&format!(
            "<li><strong>{}</strong> (effective {}): {}</li>",
            escape(&v.number),
            escape(&store::long_date(&v.effective_at)),
            escape(&v.summary)
        ));
    }
    text.push_str(&format!("\nRead every change: {url}\n"));
    let html = format!(
        "<p>We have updated our {}. Version {} takes effect on {}.</p><p>What changed since version {}:</p><ul>{items}</ul><p><a href=\"{}\">Read every change</a></p>",
        escape(&doc.title),
        escape(&to.number),
        escape(&effective),
        escape(from),
        escape(url)
    );
    Composed {
        subject,
        text,
        html,
    }
}

/// The compare address for `from` to `to` from the page's pattern.
fn link(pattern: &str, base: &str, doc: &Document, from: &str, to: &str) -> String {
    pattern
        .replace("{base}", base.trim_end_matches('/'))
        .replace("{slug}", &doc.slug)
        .replace("{from}", from)
        .replace("{to}", to)
}

struct Plan {
    doc: Document,
    versions: Vec<Version>,
    to: Version,
    behind: Vec<Accepted>,
}

async fn plan(ctx: &RouteCtx, id: i64, to: Option<&str>) -> Result<Option<Plan>, sqlx::Error> {
    let Some(doc) = store::document(ctx.db(), id).await? else {
        return Ok(None);
    };
    let versions = store::versions(ctx.db(), id).await?;
    let chosen = match to {
        Some(n) => versions.iter().find(|v| v.number == n),
        // The latest material version, else the latest.
        None => versions.iter().find(|v| v.material).or(versions.first()),
    };
    let Some(to) = chosen.cloned() else {
        return Ok(Some(Plan {
            doc,
            versions,
            to: Version {
                id: 0,
                document_id: id,
                number: String::new(),
                effective_at: String::new(),
                published_at: String::new(),
                published_by: String::new(),
                summary: String::new(),
                material: false,
                body: String::new(),
                content_hash: String::new(),
                pdf_media_id: None,
            },
            behind: Vec::new(),
        }));
    };
    let latest = store::latest_acceptances(ctx.db(), id).await?;
    let behind = store::behind(&latest, &versions, &to)
        .into_iter()
        .cloned()
        .collect();
    Ok(Some(Plan {
        doc,
        versions,
        to,
        behind,
    }))
}

/// The send page: which version to announce, who is behind it, the message
/// they would get, and the send.
pub(crate) async fn page(
    ctx: &RouteCtx,
    tools: &str,
    id: i64,
    shell: Shell,
    q: &HashMap<String, String>,
) -> Response {
    let plan = match plan(ctx, id, q.get("to").map(String::as_str)).await {
        Ok(Some(p)) => p,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failed(e),
    };
    let pattern = q
        .get("compare")
        .cloned()
        .unwrap_or_else(|| COMPARE.to_string());
    let mut body = String::new();
    if plan.versions.is_empty() {
        body.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&shell.t("Nothing published yet."))
        ));
        return shell.page_with(
            PageHead::new(t!("Send changes")).back(t!("Back"), back(ctx, id)),
            Markup::raw_unchecked(body),
        );
    }
    let with_email: Vec<&Accepted> = plan.behind.iter().filter(|a| a.email.is_some()).collect();
    let without = plan.behind.len() - with_email.len();
    let options: String = plan
        .versions
        .iter()
        .map(|v| {
            format!(
                "<option value=\"{n}\"{sel}>{n}{m}, effective {e}</option>",
                n = escape(&v.number),
                sel = if v.id == plan.to.id { " selected" } else { "" },
                m = if v.material { " (material)" } else { "" },
                e = escape(&store::long_date(&v.effective_at)),
            )
        })
        .collect();
    body.push_str(&format!(
        r#"<div class="lat-card lat-card--form" style="margin-bottom:16px"><form method="get"><div class="lat-fields">
<div class="lat-field lat-field--4"><label class="lat-field__label" for="rm-to">{l_to}</label><select class="lat-input" id="rm-to" name="to">{options}</select></div>
<div class="lat-field lat-field--8"><label class="lat-field__label" for="rm-compare">{l_compare}</label><input class="lat-input" id="rm-compare" name="compare" value="{pattern}"><p class="lat-field__help">{h_compare}</p></div>
</div><div class="lat-actions"><button class="lat-btn lat-btn--secondary" type="submit">{l_update}</button></div></form></div>"#,
        l_to = escape(&shell.t("Announce version")),
        l_compare = escape(&shell.t("Compare page address")),
        pattern = escape(&pattern),
        h_compare = escape(&shell.t("{base} is the site, {slug} the document, {from} and {to} the versions.")),
        l_update = escape(&shell.t("Update")),
    ));
    if !plan.to.material {
        body.push_str(&format!("<p class=\"lat-note\">{}</p>", escape(&shell.t("This version is not marked material; people are usually told only of material changes."))));
    }
    body.push_str(&format!(
        "<div class=\"lat-card\" style=\"padding:18px 20px;margin-bottom:16px\"><p><strong>{}</strong></p>",
        escape(&format!(
            "{} behind version {}: {} with an email, {} without.",
            plan.behind.len(),
            plan.to.number,
            with_email.len(),
            without
        ))
    ));
    if without > 0 {
        body.push_str(&format!(
            "<p class=\"lat-note\">{}</p>",
            escape(&shell.t("People without an email cannot be told by mail; the public compare page shows them the changes when they visit."))
        ));
    }
    if let Some(first) = with_email.first() {
        let url = link(
            &pattern,
            ctx.base_url(),
            &plan.doc,
            &first.version_number,
            &plan.to.number,
        );
        let m = compose(
            &plan.doc,
            &plan.versions,
            &first.version_number,
            &plan.to,
            &url,
        );
        body.push_str(&format!(
            "<h3 class=\"lat-field__heading\">{}</h3><p><strong>{}</strong></p><div class=\"rm-legal-diff\">{}</div>",
            escape(&format!("Preview, for someone on version {}", first.version_number)),
            escape(&m.subject),
            m.html
        ));
    }
    body.push_str("</div>");
    if !with_email.is_empty() {
        body.push_str(&format!(
            r#"<form method="post" action="{action}"><input type="hidden" name="_csrf" value="{csrf}"><input type="hidden" name="to" value="{to}"><input type="hidden" name="compare" value="{pattern}"><button class="lat-btn lat-btn--primary" type="submit" data-lat-confirm="{confirm}">{label}</button></form>"#,
            action = escape(&format!("{tools}/documents/{id}/notify")),
            csrf = escape(shell.csrf_token()),
            to = escape(&plan.to.number),
            pattern = escape(&pattern),
            confirm = escape(&format!("Send to {} people?", with_email.len())),
            label = escape(&format!("Send to {} people", with_email.len())),
        ));
    }
    let failed = store::failed_notices(ctx.db(), id)
        .await
        .unwrap_or_default();
    if !failed.is_empty() {
        body.push_str(&format!(
            r#"<form method="post" action="{action}" style="margin-top:12px"><input type="hidden" name="_csrf" value="{csrf}"><input type="hidden" name="compare" value="{pattern}"><button class="lat-btn lat-btn--secondary" type="submit">{label}</button></form>"#,
            action = escape(&format!("{tools}/documents/{id}/notify/retry")),
            csrf = escape(shell.csrf_token()),
            pattern = escape(&pattern),
            label = escape(&format!("Retry {} failed", failed.len())),
        ));
    }
    shell.page_with(
        PageHead::new(laterite_core::Text::dynamic(format!(
            "{}: {}",
            shell.t("Send changes"),
            plan.doc.title
        )))
        .back(t!("Back"), back(ctx, id))
        .assets(&[crate::admin::DIFF_CSS]),
        Markup::raw_unchecked(body),
    )
}

fn back(ctx: &RouteCtx, id: i64) -> String {
    format!(
        "{}/rainmill/legal/documents/{id}/edit",
        ctx.admin_path().trim_end_matches('/')
    )
}

/// Hands one message per person behind the version to the mail queue and
/// records each as a notice.
pub(crate) async fn send(
    ctx: &RouteCtx,
    id: i64,
    user: &AuthenticatedUser,
    session: &SessionHandle,
    form: &HashMap<String, String>,
) -> Result<(), Box<Response>> {
    let plan = match plan(ctx, id, form.get("to").map(String::as_str)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Err(Box::new(StatusCode::NOT_FOUND.into_response())),
        Err(e) => return Err(Box::new(failed(e))),
    };
    let pattern = form
        .get("compare")
        .cloned()
        .unwrap_or_else(|| COMPARE.to_string());
    let (mut queued, mut refused) = (0, 0);
    for who in &plan.behind {
        let Some(email) = &who.email else { continue };
        let url = link(
            &pattern,
            ctx.base_url(),
            &plan.doc,
            &who.version_number,
            &plan.to.number,
        );
        let m = compose(
            &plan.doc,
            &plan.versions,
            &who.version_number,
            &plan.to,
            &url,
        );
        let outcome = ctx
            .mail()
            .send(Message::raw(m.subject, m.text, Some(m.html)).to(email))
            .await;
        let error = outcome.err().map(|e| e.to_string());
        if error.is_some() {
            refused += 1;
        } else {
            queued += 1;
        }
        let status = if error.is_some() { "failed" } else { "queued" };
        if let Err(e) = store::record_notice(
            ctx.db(),
            &plan.doc,
            &plan.to,
            who,
            email,
            status,
            error.as_deref(),
        )
        .await
        {
            return Err(Box::new(failed(e)));
        }
    }
    ctx.audit(
        user,
        "rainmill.legal.notify",
        Some(crate::DOCUMENTS),
        Some(&id.to_string()),
        Some(&format!("{} {}", plan.doc.title, plan.to.number)),
        Some(&format!("{queued} queued, {refused} failed")),
    )
    .await;
    if refused > 0 {
        session.push_flash(
            FlashLevel::Error,
            laterite_core::Text::dynamic(format!(
                "Queued {queued} messages; {refused} could not be queued. Retry them from Send changes."
            )),
        );
    } else {
        session.push_flash(
            FlashLevel::Success,
            laterite_core::Text::dynamic(format!(
                "Queued {queued} messages. The mail log shows each delivery."
            )),
        );
    }
    Ok(())
}

/// Hands every failed notice of document `id` to the mail queue again.
pub(crate) async fn retry(
    ctx: &RouteCtx,
    id: i64,
    session: &SessionHandle,
    form: &HashMap<String, String>,
) {
    let Ok(Some(doc)) = store::document(ctx.db(), id).await else {
        return;
    };
    let versions = store::versions(ctx.db(), id).await.unwrap_or_default();
    let pattern = form
        .get("compare")
        .cloned()
        .unwrap_or_else(|| COMPARE.to_string());
    let (mut queued, mut refused) = (0, 0);
    for n in store::failed_notices(ctx.db(), id)
        .await
        .unwrap_or_default()
    {
        let Some(to) = versions.iter().find(|v| v.id == n.to_version_id) else {
            continue;
        };
        let url = link(&pattern, ctx.base_url(), &doc, &n.from_number, &to.number);
        let m = compose(&doc, &versions, &n.from_number, to, &url);
        let error = ctx
            .mail()
            .send(Message::raw(m.subject, m.text, Some(m.html)).to(&n.email))
            .await
            .err()
            .map(|e| e.to_string());
        if error.is_some() {
            refused += 1;
        } else {
            queued += 1;
        }
        let _ = store::retried(ctx.db(), n.id, error.as_deref()).await;
    }
    session.push_flash(
        if refused > 0 {
            FlashLevel::Error
        } else {
            FlashLevel::Success
        },
        laterite_core::Text::dynamic(format!(
            "Retried: {queued} queued, {refused} still failing."
        )),
    );
}
