//! The metadata export is a *faithful projection* of the registries.
//!
//! The gap this file exists to close: nothing previously asserted that the
//! export described what the registries actually hold — only that it matched
//! its own generator, which proves nothing about a registry silently dropped or
//! a field silently mis-copied.
//!
//! Every test here runs the real `export-docs` binary and reads the file it
//! wrote, so what is checked is the artifact a caller gets, not a re-derivation
//! of it.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

/// Run the exporter into a temp path and parse what it wrote.
fn export() -> (Value, usize) {
    let dir = std::env::temp_dir().join(format!("darkly-docs-test-{}", std::process::id()));
    let out = dir.join("metadata.json");
    let _ = std::fs::remove_dir_all(&dir);

    let status = Command::new(env!("CARGO_BIN_EXE_export-docs"))
        .arg("--out")
        .arg(&out)
        .status()
        .expect("failed to run export-docs");
    assert!(status.success(), "export-docs exited with {status}");

    let text = std::fs::read_to_string(&out).expect("export-docs wrote no file");
    let json = serde_json::from_str(&text).expect("export-docs wrote invalid JSON");
    let len = text.len();
    let _ = std::fs::remove_dir_all(&dir);
    (json, len)
}

fn catalogs_of(json: &Value) -> &Vec<Value> {
    json["catalogs"]
        .as_array()
        .expect("catalogs is not an array")
}

fn catalog<'a>(json: &'a Value, id: &str) -> &'a Value {
    catalogs_of(json)
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no catalog `{id}` in the export"))
}

/// Every registry directory `build.rs` scanned produced exactly one catalog.
///
/// This compares the export against what the build *found on disk*, not against
/// a second hand-written list. Adding an eighth registry directory therefore
/// cannot be forgotten in two places at once: the same generated list feeds
/// `catalogs()` and this assertion.
#[test]
fn every_catalog_source_is_exported() {
    let (json, _) = export();
    let exported: BTreeSet<&str> = catalogs_of(&json)
        .iter()
        .filter_map(|c| c["id"].as_str())
        .filter(|id| !id.starts_with("settings."))
        .collect();

    let scanned: Vec<_> = darkly::catalog::catalog_sources();
    assert!(!scanned.is_empty(), "build.rs scanned no catalog sources");

    for source in &scanned {
        assert!(
            exported.contains(source.id),
            "registry directory `{}` produces catalog `{}`, which the export omits",
            source.dir,
            source.id
        );
    }
    assert_eq!(
        exported.len(),
        scanned.len(),
        "the export carries a registry catalog no scanned directory produces: {exported:?} vs {:?}",
        scanned.iter().map(|s| s.id).collect::<Vec<_>>()
    );
}

/// Every entry's fields equal the registration's, field by field.
///
/// A type-id *set* comparison would pass a `catalog_entry()` that returned
/// `display_name` in `description`; this would not.
#[test]
fn export_is_a_faithful_projection() {
    let (json, _) = export();

    /// Assert one catalog's entries against `(type_id, display_name, icon,
    /// description, category, hotkey_action)` read off the registrations.
    type Row = (
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    );
    fn check(json: &Value, id: &str, want: Vec<Row>) {
        let cat = catalog(json, id);
        let entries = cat["entries"].as_array().unwrap();
        assert_eq!(
            entries.len(),
            want.len(),
            "catalog `{id}` exports {} entries, the registry holds {}",
            entries.len(),
            want.len()
        );
        for (e, (type_id, display_name, icon, description, category, hotkey)) in
            entries.iter().zip(want)
        {
            let f = |k: &str| e[k].as_str();
            assert_eq!(f("type"), Some(type_id), "`{id}` type mismatch");
            assert_eq!(
                f("displayName"),
                Some(display_name),
                "`{id}/{type_id}` displayName"
            );
            assert_eq!(f("icon"), icon, "`{id}/{type_id}` icon");
            assert_eq!(
                f("description"),
                description,
                "`{id}/{type_id}` description"
            );
            assert_eq!(f("category"), category, "`{id}/{type_id}` category");
            assert_eq!(f("hotkeyAction"), hotkey, "`{id}/{type_id}` hotkeyAction");
        }
    }

    let some = |s: &'static str| (!s.is_empty()).then_some(s);

    check(
        &json,
        "filters",
        darkly::gpu::filter::FilterPipelineRegistry::new()
            .types()
            .into_iter()
            .map(|r| {
                (
                    r.type_id,
                    r.display_name,
                    some(r.icon),
                    some(r.description),
                    None,
                    None,
                )
            })
            .collect(),
    );

    check(
        &json,
        "veils",
        darkly::gpu::veil::VeilRegistry::new()
            .types()
            .into_iter()
            .map(|r| {
                (
                    r.type_id,
                    r.display_name,
                    None,
                    some(r.description),
                    None,
                    None,
                )
            })
            .collect(),
    );

    check(
        &json,
        "voids",
        darkly::gpu::void::VoidRegistry::new()
            .types()
            .into_iter()
            .map(|r| {
                (
                    r.type_id,
                    r.display_name,
                    some(r.icon),
                    some(r.description),
                    None,
                    None,
                )
            })
            .collect(),
    );

    check(
        &json,
        "blendModes",
        darkly::gpu::blend_mode::registry()
            .all()
            .into_iter()
            .map(|r| {
                (
                    r.type_id,
                    r.display_name,
                    None,
                    some(r.description),
                    some(r.category),
                    None,
                )
            })
            .collect(),
    );

    check(
        &json,
        "tools",
        darkly::tool::registry()
            .types()
            .into_iter()
            .map(|r| {
                (
                    r.type_id,
                    r.display_name,
                    some(r.icon),
                    some(r.description),
                    None,
                    some(r.hotkey_action),
                )
            })
            .collect(),
    );

    check(
        &json,
        "layerKinds",
        darkly::document::layer_kind::registry()
            .all()
            .into_iter()
            .map(|r| {
                (
                    r.type_id,
                    r.display_name,
                    some(r.icon),
                    some(r.description),
                    None,
                    None,
                )
            })
            .collect(),
    );

    check(
        &json,
        "layerFilters",
        darkly::document::filter::registry()
            .all()
            .into_iter()
            .map(|r| {
                (
                    r.type_id,
                    r.display_name,
                    some(r.icon),
                    some(r.description),
                    None,
                    None,
                )
            })
            .collect(),
    );

    check(
        &json,
        "brushes",
        darkly::brush::builtin_brushes::docs()
            .iter()
            .map(|(stem, info)| {
                (
                    *stem,
                    info.name.as_str(),
                    info.icon,
                    some(info.description.as_str()),
                    some(info.category.as_str()),
                    None,
                )
            })
            .collect(),
    );

    check(
        &json,
        "brushNodes",
        darkly::brush::registry()
            .types()
            .into_iter()
            .map(|r| {
                (
                    r.node.type_id,
                    r.node.display_name,
                    // Ports are not parameters and a preview-fallback glyph is
                    // not a palette icon — both deliberately absent.
                    None,
                    some(r.node.description),
                    some(r.node.category),
                    None,
                )
            })
            .collect(),
    );

    // The capture kind is voids' alone, and it rides beside the previewability
    // every catalog now answers.
    for r in darkly::gpu::void::VoidRegistry::new().types() {
        let e = catalog(&json, "voids")["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == r.type_id)
            .unwrap();
        let want_capture = r.capture_kind.map(|k| serde_json::to_value(k).unwrap());
        let got = e["captureKind"].clone();
        let got = (!got.is_null()).then_some(got);
        assert_eq!(got, want_capture, "`voids/{}` captureKind", r.type_id);
    }

    // Previewability is one question every catalog answers, and the artifact
    // must carry the same answer the registry gives. A consumer pairing this
    // JSON with a directory of rendered assets reads exactly this flag to know
    // which entries it should find one for.
    let mut previewable = 0usize;
    for cat in catalogs_of(&json) {
        for e in cat["entries"].as_array().unwrap() {
            let got = e["supportsPreview"].as_bool();
            assert!(
                got.is_some(),
                "`{}/{}` has no supportsPreview",
                cat["id"],
                e["type"]
            );
            previewable += usize::from(got == Some(true));
        }
    }
    assert_eq!(
        previewable, 34,
        "7 filters + 10 veils + 1 void + 16 blend modes declare a preview recipe"
    );

    // Settings ride on the same footing, against the section schema minus the
    // prefs the UI does not treat as settings.
    for section in darkly::config::sections::registrations() {
        let cat = catalog(&json, &format!("settings.{}", section.id));
        assert_eq!(cat["title"].as_str(), Some(section.display_name));
        assert_eq!(cat["order"].as_i64(), Some(section.order as i64));
        let params = cat["entries"][0]["params"].as_array().unwrap();
        let want: Vec<_> = section
            .prefs
            .iter()
            .filter(|p| !matches!(p.widget, darkly::config::schema::WidgetHint::Hidden))
            .collect();
        assert_eq!(
            params.len(),
            want.len(),
            "settings.{} exports {} prefs, the section declares {} visible",
            section.id,
            params.len(),
            want.len()
        );
        for (got, pref) in params.iter().zip(want) {
            assert_eq!(
                got["name"].as_str(),
                Some(pref.key),
                "settings.{}",
                section.id
            );
            assert_eq!(got["label"].as_str(), Some(pref.display_name));
            assert_eq!(got["description"].as_str(), pref.description);
            assert_ne!(got["widget"].as_str(), Some("hidden"));
        }
    }
}

/// Every entry's exported `params` equals `ParamInfo` over the registration's
/// own `&'static [ParamDef]`, in order — so a parameter cannot be dropped,
/// reordered, or re-derived at the exporter.
#[test]
fn params_match_the_registration_slice() {
    let (json, _) = export();

    fn check(
        json: &Value,
        cat_id: &str,
        type_id: &str,
        defs: &'static [darkly::gpu::params::ParamDef],
    ) {
        let entry = catalog(json, cat_id)["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == type_id)
            .unwrap_or_else(|| panic!("no `{cat_id}/{type_id}` in the export"));
        let got = entry["params"].as_array().unwrap();
        // Round-trip the expectation through a JSON *string* as well: an `f32`
        // default reaches `Value` as its widened `f64` (0.299 → 0.29899999…)
        // unless it goes through the same textual formatting the file did.
        let want: Value = serde_json::from_str(
            &serde_json::to_string(
                &defs
                    .iter()
                    .map(|d| darkly::engine::ParamInfo::from_def(d, None))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            &Value::Array(got.clone()),
            &want,
            "`{cat_id}/{type_id}` params differ from its registration slice"
        );
    }

    let mut checked = 0;
    for r in darkly::gpu::filter::FilterPipelineRegistry::new().types() {
        check(&json, "filters", r.type_id, r.params);
        checked += 1;
    }
    for r in darkly::gpu::veil::VeilRegistry::new().types() {
        check(&json, "veils", r.type_id, r.params);
        checked += 1;
    }
    for r in darkly::gpu::void::VoidRegistry::new().types() {
        check(&json, "voids", r.type_id, r.params);
        checked += 1;
    }
    for r in darkly::tool::registry().types() {
        check(&json, "tools", r.type_id, r.params);
        checked += 1;
    }
    assert!(checked > 0, "no parameterized registries were checked");
}

/// The artifact carries values, not bytes. A rendered preview belongs in the
/// asset directory a sibling binary writes, keyed by the same `version`.
#[test]
fn manifest_carries_no_binary_payload() {
    let (json, _) = export();

    fn walk(v: &Value, path: &str) {
        match v {
            Value::String(s) => {
                assert!(
                    !s.starts_with("data:"),
                    "{path} carries a data: URI — the artifact must not embed bytes"
                );
                // Anything this long in a metadata field is a payload, not a
                // label or a sentence.
                assert!(
                    s.len() < 4096,
                    "{path} carries a {}-byte string — too long to be metadata",
                    s.len()
                );
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, &format!("{path}[{i}]"));
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    walk(x, &format!("{path}.{k}"));
                }
            }
            _ => {}
        }
    }
    walk(&json, "$");
}

#[test]
fn manifest_under_256_kb() {
    let (_, len) = export();
    assert!(
        len < 256 * 1024,
        "the manifest is {len} bytes; over 256 KB means something is riding along that should not be"
    );
}
