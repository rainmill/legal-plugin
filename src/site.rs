//! The site: the `rainmill.legal.document` component shows a document's version
//! in force (or one the URL names), its history, and what changed between two
//! versions or two dates.
//!
//! The page's URL parameters choose what it shows: `version` names a version;
//! `from` and `to` (each a version number or a `YYYY-MM-DD` date, a date meaning
//! the version in force that day) compare two. A version carries `toc`, its
//! sections by heading, for an "On this page" list. A theme gives them whatever URL
//! shape it likes: `/legal/:slug/:version?`, `/legal/:slug/compare/:from/:to`.

use laterite_core::serde_json;
use laterite_core::sqlx;
use laterite_web::theme::component::{
    Component, ComponentContext, ComponentOutput, PropertyDef, PropertyKind,
};
use laterite_web::theme::ThemeError;
use rainmill_pdf::history::word_diff;
use serde_json::{json, Value};

use crate::store::{self, Version};

/// The component's markup when the theme has none of its own.
const MARKUP: &str = r#"{% set d = component %}<article class="legal-document">
<header class="legal-document__head"><h1>{{ d.document.title }}</h1>
{% if d.version %}<p class="legal-document__meta">Version {{ d.version.number }} · Effective {{ d.version.effective }}{% if d.version.pdf %} · <a href="{{ d.version.pdf }}" target="_blank" rel="noopener">PDF</a>{% endif %}</p>{% endif %}</header>
{% if d.missing %}<p class="legal-document__missing">There is no version {{ d.missing }} of this document.</p>
{% elif d.compare %}<section class="legal-document__compare"><h2>What changed from version {{ d.compare.from.number }} to {{ d.compare.to.number }}</h2>
<ul class="legal-document__changes">{% for c in d.compare.changes %}<li><strong>{{ c.number }}</strong>, effective {{ c.effective }}: {{ c.summary }}</li>{% endfor %}</ul>
<div class="legal-document__diff">{{ d.compare.html|safe }}</div></section>
{% elif d.version %}<div class="legal-document__body">{{ d.version.html|safe }}</div>
{% else %}<p class="legal-document__missing">This document has not been published yet.</p>{% endif %}
{% if d.history %}<section class="legal-document__history"><h2>History</h2><ol>{% for v in d.history %}<li><a href="{{ v.url }}">Version {{ v.number }}</a>, effective {{ v.effective }}{% if v.current %} (in force){% elif v.scheduled %} (from {{ v.effective }}){% endif %}: {{ v.summary }}</li>{% endfor %}</ol></section>{% endif %}
</article>"#;

/// The `rainmill.legal.document` component.
pub(crate) struct DocumentComponent;

#[laterite_core::strata::async_trait]
impl Component for DocumentComponent {
    fn properties(&self) -> Vec<PropertyDef> {
        let mut slug = PropertyDef::new("slug", "Document address", PropertyKind::Text);
        slug.required = true;
        let mut url = PropertyDef::new("url", "Page address", PropertyKind::Text);
        url.default = Some(json!("/legal/{slug}"));
        vec![slug, url]
    }

    fn markup(&self) -> Option<&'static str> {
        Some(MARKUP)
    }

    async fn run(&self, cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError> {
        let text = |k: &str| {
            cx.properties
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let slug = text("slug").unwrap_or_default();
        let fail = |e: sqlx::Error| ThemeError::Config(e.to_string());
        let Some(doc) = store::document_by_slug(cx.db, &slug).await.map_err(fail)? else {
            return Ok(ComponentOutput::data(json!({ "document": null })));
        };
        let versions = store::versions(cx.db, doc.id).await.map_err(fail)?;
        let base = text("url")
            .unwrap_or_else(|| "/legal/{slug}".into())
            .replace("{slug}", &doc.slug);
        let base = base.trim_end_matches('/').to_string();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let current = store::in_force(&versions, &now).map(|v| v.id);
        let param = |k: &str| {
            cx.params
                .get(k)
                .map(String::as_str)
                .filter(|s| !s.is_empty())
        };

        let mut data = json!({
            "document": { "title": doc.title, "slug": doc.slug, "description": doc.description },
            "version": null,
            "compare": null,
            "missing": null,
            "url": base,
        });
        let history: Vec<Value> = versions
            .iter()
            .map(|v| {
                json!({
                    "number": v.number,
                    "effective": store::long_date(&v.effective_at),
                    "summary": v.summary,
                    "material": v.material,
                    "current": Some(v.id) == current,
                    "scheduled": v.effective_at > now,
                    "url": format!("{base}/{}", v.number),
                })
            })
            .collect();
        data["history"] = Value::Array(history);

        if let (Some(from), Some(to)) = (param("from"), param("to")) {
            match (resolve(&versions, from), resolve(&versions, to)) {
                (Some(a), Some(b)) => {
                    let (a, b) =
                        if (a.published_at.as_str(), a.id) <= (b.published_at.as_str(), b.id) {
                            (a, b)
                        } else {
                            (b, a)
                        };
                    let mut between: Vec<&Version> = versions
                        .iter()
                        .filter(|v| {
                            (v.published_at.as_str(), v.id) > (a.published_at.as_str(), a.id)
                                && (v.published_at.as_str(), v.id)
                                    <= (b.published_at.as_str(), b.id)
                        })
                        .collect();
                    between.sort_by(|x, y| {
                        (x.published_at.as_str(), x.id).cmp(&(y.published_at.as_str(), y.id))
                    });
                    data["compare"] = json!({
                        "from": { "number": a.number, "effective": store::long_date(&a.effective_at) },
                        "to": { "number": b.number, "effective": store::long_date(&b.effective_at) },
                        "html": word_diff(&a.body, &b.body),
                        "changes": between.iter().map(|v| json!({
                            "number": v.number,
                            "effective": store::long_date(&v.effective_at),
                            "summary": v.summary,
                        })).collect::<Vec<_>>(),
                    });
                }
                (None, _) => data["missing"] = json!(from),
                (_, None) => data["missing"] = json!(to),
            }
            return Ok(ComponentOutput::data(data));
        }

        let shown = match param("version") {
            Some(number) => match versions.iter().find(|v| v.number == number) {
                Some(v) => Some(v),
                None => {
                    data["missing"] = json!(number);
                    None
                }
            },
            None => versions.iter().find(|v| Some(v.id) == current),
        };
        if let Some(v) = shown {
            let pdf = match v.pdf_media_id {
                Some(id) => laterite_media::record::find(cx.db, id)
                    .await
                    .ok()
                    .flatten()
                    .map(|r| laterite_admin::medialib::file_url(&r))
                    .unwrap_or_default(),
                None => String::new(),
            };
            let html = laterite_core::markdown::render(&v.body);
            data["version"] = json!({
                "number": v.number,
                "effective": store::long_date(&v.effective_at),
                "summary": v.summary,
                "toc": contents(&html),
                "html": html,
                "pdf": pdf,
                "current": Some(v.id) == current,
            });
        }
        Ok(ComponentOutput::data(data))
    }
}

/// The document's sections, for an "On this page" list: each second-level
/// heading's id and words, from the rendered HTML.
fn contents(html: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<h2 id=\"") {
        let after = &rest[start + 8..];
        let Some(end_id) = after.find('"') else { break };
        let id = &after[..end_id];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let Some(close) = after.find("</h2>") else {
            break;
        };
        let inner = &after[open_end + 1..close];
        let mut text = String::new();
        let mut in_tag = false;
        for c in inner.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => text.push(c),
                _ => {}
            }
        }
        out.push(json!({ "id": id, "title": text.trim() }));
        rest = &after[close..];
    }
    out
}

/// A version by number, or the one in force at the end of a `YYYY-MM-DD` day.
fn resolve<'a>(versions: &'a [Version], wanted: &str) -> Option<&'a Version> {
    if let Some(v) = versions.iter().find(|v| v.number == wanted) {
        return Some(v);
    }
    let day = chrono::NaiveDate::parse_from_str(wanted, "%Y-%m-%d").ok()?;
    store::in_force(versions, &format!("{day}T23:59:59Z"))
}
