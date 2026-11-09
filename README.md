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

Permission | Allows
--- | ---
`rainmill.legal.edit` | Opening documents and saving drafts
`rainmill.legal.publish` | Publishing a version and rendering its PDF again
`rainmill.legal.notify` | Sending the changes between versions (coming)

## Licence

MIT or Apache-2.0, at your option.
