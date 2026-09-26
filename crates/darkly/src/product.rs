//! Loads [`product.yaml`](../../product.yaml), the authority for what Darkly
//! says it is, and projects it into the syntaxes its consumers need.
//!
//! The copy itself lives in the YAML rather than in constants here, so the rule
//! for each value sits beside it as a comment and a store-listing pass is an
//! edit to a data file. Text targets read it through a `docs_md` region
//! ([`super::docs_md::fragments`]); anything that cannot read a Rust value
//! reads [`APP_JSON`], which this module writes.

use std::path::Path;
use std::sync::OnceLock;

/// Where the machine-readable projection lands, relative to the repository
/// root. Consumed by `desktop/forge.config.js`, which is JavaScript and has no
/// YAML parser to read the source with.
pub const APP_JSON: &str = "packaging/app.json";

/// The authored source, embedded at compile time.
///
/// Embedded rather than read from disk because a `docs_md` fragment is given no
/// repository root ([`super::docs_md::FragmentCtx`]): its output is a pure
/// function of the registries and its own arguments, and reaching for a file
/// would break that on every target.
const SOURCE: &str = include_str!("../product.yaml");

/// Everything the stores, launchers and installers are told about Darkly.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    /// The one-line pitch.
    pub summary: String,
    /// Freedesktop menu categories.
    pub categories: Vec<String>,
    /// Desktop-entry search keywords.
    pub keywords: Vec<String>,
    /// The short description, one paragraph.
    pub description: String,
    /// The long description, in reading order. What the store listing shows.
    pub long_description: Vec<Block>,
}

/// One block of the long description: the subset of rich text every store
/// renders, which is AppStream's `<p>`, `<ul>` and `<em>`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum Block {
    /// A paragraph, authored as a string.
    Para(String),
    /// A bulleted list, authored as a list of strings.
    List(Vec<String>),
    /// A section title, authored as `heading: "..."`. AppStream has no heading
    /// element, so stores get it as an emphasized paragraph of its own.
    Heading { heading: String },
}

/// [`SOURCE`], parsed once.
///
/// Panics on malformed input: this is shipped data that the test suite parses
/// on every run, so a broken edit cannot reach a release.
pub fn product() -> &'static Product {
    static PRODUCT: OnceLock<Product> = OnceLock::new();
    PRODUCT.get_or_init(|| {
        serde_yaml_ng::from_str(SOURCE)
            .unwrap_or_else(|e| panic!("invalid crates/darkly/product.yaml: {e}"))
    })
}

/// Escape the five characters XML cannot carry literally in character data.
///
/// Applied on render rather than stored escaped, so the authored text stays
/// readable and a consumer that is not XML is not paying for XML's syntax.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The JSON projection, exactly as it is written to disk.
pub fn app_json() -> String {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let list = |xs: &[String]| {
        xs.iter()
            .map(|x| format!("\"{}\"", esc(x)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let product = product();
    let long_description = product
        .long_description
        .iter()
        .map(|block| match block {
            Block::Para(p) => format!("\"{}\"", esc(p)),
            Block::List(items) => format!("[{}]", list(items)),
            Block::Heading { heading } => format!("{{\"heading\": \"{}\"}}", esc(heading)),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{{\n  \"_generated\": \"cargo sync-docs writes this from \
         crates/darkly/product.yaml; do not edit\",\n  \
         \"summary\": \"{}\",\n  \"categories\": [{}],\n  \
         \"keywords\": [{}],\n  \"description\": \"{}\",\n  \
         \"long_description\": [{}]\n}}\n",
        esc(&product.summary),
        list(&product.categories),
        list(&product.keywords),
        esc(&product.description),
        long_description,
    )
}

/// Write [`APP_JSON`] under `root`, or report whether it is stale.
///
/// Returns whether the file on disk differs from what this module says. The
/// caller decides what that means: `cargo sync-docs` writes it, the test suite
/// fails on it.
pub fn sync_app_json(root: &Path, write: bool) -> Result<bool, std::io::Error> {
    let path = root.join(APP_JSON);
    let wanted = app_json();
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current == wanted {
        return Ok(false);
    }
    if write {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, wanted)?;
    }
    Ok(true)
}
