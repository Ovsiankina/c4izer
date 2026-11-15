//! End-to-end checks on every view of `tests/data/example.dsl` through
//! `pipeline::build_all`: the requirements that need place, route, label,
//! scene and export together, so no single stage can own them
//! (docs/deferred.md rows for CORE-25 and CORE-30).

use std::sync::atomic::AtomicBool;

use c4rizer_core::export::svg;
use c4rizer_core::lint::Rule;
use c4rizer_core::model::{Model, parse};
use c4rizer_core::pipeline::{AllViews, build_all};
use c4rizer_core::scene::{Scene, TextBlock};

const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/data/example.dsl"
));

fn model() -> Model {
    match parse(EXAMPLE) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    }
}

fn all(model: &Model) -> AllViews {
    build_all(model, &AtomicBool::new(false)).expect("never stopped")
}

/// Every text block a scene draws: box names, type lines and
/// descriptions, labels, boundary tags, title, subtitle, key entries.
fn texts(scene: &Scene) -> Vec<(&'static str, &TextBlock)> {
    let mut out = Vec::new();
    for b in &scene.boxes {
        out.push(("box name", &b.name));
        out.push(("box type", &b.type_line));
        if let Some(d) = &b.description {
            out.push(("box description", d));
        }
    }
    for l in &scene.labels {
        out.push(("label", &l.text));
    }
    for b in &scene.boundaries {
        out.push(("boundary tag", &b.tag));
    }
    if let Some(t) = &scene.title {
        out.push(("title", t));
    }
    if let Some(t) = &scene.subtitle {
        out.push(("subtitle", t));
    }
    for e in &scene.legend.entries {
        out.push(("key", &e.text));
    }
    out
}

/// CORE-25: each example view built twice gives byte-identical SVG
/// (`export::svg`).
#[test]
fn core_25_example_svg_bytes_identical() {
    // Two parses as well as two builds: nothing may hang on the model's
    // identity or on iteration order.
    let first = all(&model());
    let second = all(&model());
    assert_eq!(first.views.len(), 4);
    for (a, b) in first.views.iter().zip(&second.views) {
        let (sa, sb) = (svg(&a.scene), svg(&b.scene));
        assert!(sa.starts_with("<svg") || sa.starts_with("<?xml"));
        assert_eq!(sa, sb, "{}: SVG bytes differ between builds", a.plan.key);
    }
    assert_eq!(first, second);
}

/// CORE-30: every `TextBlock::fits` is true on every example view (no
/// L25 finding).
#[test]
fn core_30_example_texts_fit() {
    let views = all(&model());
    assert_eq!(views.views.len(), 4);
    for v in &views.views {
        let texts = texts(&v.scene);
        assert!(!texts.is_empty());
        for (what, t) in texts {
            assert!(t.fits, "{}: {what} {:?} does not fit", v.plan.key, t.lines);
        }
        assert!(
            v.report.findings.iter().all(|f| f.rule != Rule::TextFit),
            "{}: {:#?}",
            v.plan.key,
            v.report.findings
        );
    }
}
