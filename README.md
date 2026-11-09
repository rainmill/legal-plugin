# Rainmill Legal

Versioned legal documents for [Laterite](https://laterite.rs) applications: terms, privacy policies and anything else that changes over time and must say what it said when. Module id `rainmill.legal`; requires `rainmill.pdf`.

```rust
// main.rs
Bootstrap::new("config")
    .modules(plugins_manifest::all()) // rainmill-pdf and rainmill-legal under plugins/
    .run()
    .await
```

## Documents and drafts

The admin gains **Legal** with **Documents**. A document has a title, an address (`privacy`), a numbering scheme, a PDF layout and a draft. The draft is edited beside a live PDF of itself and is never public; saving keeps the draft.

Field | Holds
--- | ---
Title | The document's name, also the PDF title.
Address | Its key on the site and in code (`privacy`).
Numbering | How versions are numbered (below).
PDF layout | A layout from PDF Templates; the default layout when empty.
Draft | Markdown, taken verbatim: `{{` stays as written.

## Draft history and undo

Every save of a document keeps its state (title, description, layout, draft), the latest fifty, with who saved it. None of it is public.

Action | Does
--- | ---
**Undo last save** | Brings back the draft before the latest save; again, the one before that, and on. Asks first, since unsaved edits in the editor go too.
**Draft history** | Every kept state newest first, with only the paragraphs each save changed, marked word by word.
**Restore** | Brings back a chosen state.

An undo or restore is kept as a state of its own, so nothing is lost by using them, and a save is never a version: only **Publish new version** makes one.

```rust
let states = rainmill_legal::drafts::list(&db, doc.id).await?;
rainmill_legal::drafts::undo(&db, doc.id, "ada").await?;
rainmill_legal::drafts::restore(&db, doc.id, states[2].id, "ada").await?;
```

## Publishing

**Publish new version** shows the changes since the latest version, then asks for:

Field | Notes
--- | ---
Version number | Empty takes the scheme's next number.
Takes effect | A date, possibly in the future; the version in force stays until then.
Material change | Only material changes are sent to people.
What changed | Required; shown with the version and in any message about it.

Scheme | Next number
--- | ---
Major and minor | `1.0` first; material `2.0`, otherwise `1.1`.
Effective date | `2026-11-09`, then `2026-11-09.2` the same day.
Free text | Whatever the author types.

Publishing writes a version that never changes, with a BLAKE3 hash of its text, renders it as a PDF/A file in the document's layout, stores that in the media library linked to the version, and writes an audit entry (`rainmill.legal.version.publish`). Refused: an empty draft, a draft unchanged since the latest version, a number already used, no summary. If the PDF cannot be stored, the version is still published and **Render PDF** on the Versions page tries again.

**Versions** lists every version newest first: number, effective date, who published it and when, what changed, In force or Scheduled, Material, and its PDF.

```rust
use rainmill_legal::store;

let doc = store::document_by_slug(&db, "privacy").await?.unwrap();
let versions = store::versions(&db, doc.id).await?;
let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
let current = store::in_force(&versions, &now);
```

## On the site

Attach the `rainmill.legal.document` component to pages whose URLs carry the parameters it reads:

```ini
# pages/legal.htm
url = "/legal/:slug/:version?"

[rainmill.legal.document legal]
slug = "{{ :slug }}"
url = "/legal/{slug}"
==
{{ component("legal") }}
```

```ini
# pages/legal-compare.htm
url = "/legal/:slug/compare/:from/:to"

[rainmill.legal.document legal]
slug = "{{ :slug }}"
==
{{ component("legal") }}
```

URL parameter | Shows
--- | ---
none | The version in force, with its PDF link
`version` | That version (`/legal/privacy/1.0`)
`from`, `to` | What changed between them: each a version number or a `YYYY-MM-DD` date, a date meaning the version in force that day, with every version's summary in between

The history lists every version newest first, marking the one in force and any scheduled for later. The data is `component.document`, `.version` (number, effective, summary, html, pdf), `.history`, `.compare` (from, to, html, changes) and `.missing` (a version the URL named that does not exist), for a theme that writes its own markup.

## Acceptances

A form that shows a document records that the person accepted the version in force:

```rust
use rainmill_legal::{accept, Method};

let version = accept(&db, "privacy", ("launch_signup", &id.to_string()), Method::Form, email).await?;
```

Case | Result
--- | ---
A version is in force | An acceptance row (document, version and its number, the subject, email, method, time); the version is returned.
No document has the address | `AcceptError::NoDocument`.
Nothing is in force yet | `AcceptError::NothingInForce`.

**Acceptances** in the admin lists every row, searchable and exportable.

## Sending the changes

**Send changes** on a document picks a version to announce (the latest material one by default) and finds everyone whose latest acceptance is of an older version. It counts those with and without an email, previews the message (each change's summary since the person's version, and a link to the public compare page), and sends one message per person through the application's mail queue.

Case | Result
--- | ---
Handed to the mail queue | A notice, `queued`; the mail log shows the delivery, and the Jobs list holds a message that finally failed, with Retry.
The queue refused it | A notice, `failed`, with the error; **Retry failed** on Send changes hands those to the queue again.
No email on record | Counted on the page, never sent; the public compare page shows them the changes.

**Notices** in the admin lists every notice with its status, attempts and last error. Each send writes an audit entry (`rainmill.legal.notify`).

Compare page address | Default
--- | ---
The link in each message | `{base}/legal/{slug}/compare/{from}/{to}`, editable on the page

Permission | Allows
--- | ---
`rainmill.legal.edit` | Opening documents, saving drafts, and the acceptances list
`rainmill.legal.publish` | Publishing a version and rendering its PDF again
`rainmill.legal.notify` | Sending the changes and the notices list

## Licence

MIT or Apache-2.0, at your option.
