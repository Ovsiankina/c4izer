//! P2-105 and P2-106: the lattice pitch of the layered placement grows
//! where a label needs room, and only there. Built through
//! `pipeline::build_spec` from DSL text with `Keep::default()`, so the
//! label sizes are the ones `label::place_labels` gives.

use std::sync::atomic::AtomicBool;

use c4rizer_core::geom::{GRID_PX, Point};
use c4rizer_core::model::{Model, parse};
use c4rizer_core::pipeline::{ViewBuild, build_spec};
use c4rizer_core::place::Keep;
use c4rizer_core::view::EdgeId;

fn build(model_body: &str, view: &str) -> (Model, ViewBuild) {
    let src = format!(
        "workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{view}\n    }}\n}}\n"
    );
    let model = match parse(&src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    };
    let b = build_spec(
        &model,
        &model.views[0],
        &Keep::default(),
        &AtomicBool::new(false),
    )
    .expect("never stopped");
    (model, b)
}

/// The box index of the element keyed `key`.
fn index(m: &Model, b: &ViewBuild, key: &str) -> usize {
    b.plan
        .boxes
        .iter()
        .position(|x| {
            m.element(x.element)
                .key
                .as_ref()
                .is_some_and(|k| k.value == key)
        })
        .unwrap_or_else(|| panic!("no box {key}"))
}

/// Box `i`'s rectangle in px: (min x, min y, max x, max y).
fn px(b: &ViewBuild, i: usize) -> (i32, i32, i32, i32) {
    let r = b.plan.boxes[i].rect_at(b.routes.centres[i]);
    (
        r.min.x * GRID_PX,
        r.min.y * GRID_PX,
        r.max.x * GRID_PX,
        r.max.y * GRID_PX,
    )
}

/// CORE-14 bound of P2-105 for a label extent `e` px: `max(30, e/2) +
/// max(50, e/2)`.
fn bound(e: i32) -> i32 {
    30.max(e / 2) + 50.max(e / 2)
}

fn label_size(b: &ViewBuild, edge: usize) -> (i32, i32) {
    let l = b
        .labels
        .label(EdgeId(u32::try_from(edge).expect("small")))
        .expect("edge has a label");
    (l.width_px, l.height_px)
}

const TALL: &str = "Sends every nightly batch of ledger entries and account records through the shared queue for archival";

#[test]
fn p2_105_gap_fits_label() {
    // (a) Two boxes on consecutive rows, a tall label between them.
    let (m, vb) = build(
        &format!(
            r#"        sys = softwareSystem "System" "Does things." {{
            a = container "A" "First." "Rust"
            c = container "C" "Second." "Rust"
        }}
        a -> c "{TALL}"
"#
        ),
        "        container sys \"V\" {\n            include *\n            labelWidth 10\n        }",
    );
    let (top, bottom) = (index(&m, &vb, "a"), index(&m, &vb, "c"));
    let (_, tall) = label_size(&vb, 0);
    assert!(
        bound(tall) > 140,
        "label {tall}px tall does not test growth"
    );
    let gap = px(&vb, bottom).1 - px(&vb, top).3;
    assert!(
        gap >= bound(tall),
        "vertical gap {gap}px, label needs {}",
        bound(tall)
    );

    // (b) Two people on the top row, a wide label between them, both
    // feeding one box below.
    let (m, vb) = build(
        r#"        p = person "Operator" "Runs things."
        q = person "Auditor" "Checks things."
        sys = softwareSystem "System" "Does things."
        p -> q "Hands every finished nightly report over in person for a second signature"
        p -> sys "Runs"
        q -> sys "Checks"
"#,
        "        systemContext sys \"V\" {\n            include *\n            labelWidth 60\n        }",
    );
    let (lead, peer) = (index(&m, &vb, "p"), index(&m, &vb, "q"));
    let (wide, _) = label_size(&vb, 0);
    assert!(
        bound(wide) > 140,
        "label {wide}px wide does not test growth"
    );
    assert_eq!(vb.routes.centres[lead].y, vb.routes.centres[peer].y);
    let (rp, rq) = (px(&vb, lead), px(&vb, peer));
    let gap = (rq.0 - rp.2).max(rp.0 - rq.2);
    assert!(
        gap >= bound(wide),
        "horizontal gap {gap}px, label needs {}",
        bound(wide)
    );
}

#[test]
fn p2_106_pitch_grows_only_where_needed() {
    // Three rows; only the a -> c gap carries a tall label.
    let (m, vb) = build(
        &format!(
            r#"        sys = softwareSystem "System" "Does things." {{
            a = container "A" "First." "Rust"
            c = container "C" "Second." "Rust"
            d = container "D" "Third." "Rust"
        }}
        a -> c "{TALL}"
        c -> d "Feeds"
"#
        ),
        "        container sys \"V\" {\n            include *\n            labelWidth 10\n        }",
    );
    let at = |k: &str| vb.routes.centres[index(&m, &vb, k)];
    let (ya, yc, yd) = (at("a"), at("c"), at("d"));
    assert!(ya.y < yc.y && yc.y < yd.y, "{ya:?} {yc:?} {yd:?}");
    assert!(
        yc.y - ya.y > 28,
        "tall label gap not grown: {}",
        yc.y - ya.y
    );
    assert_eq!(yd.y - yc.y, 28, "short label gap grew");
    let all: &[Point] = &vb.routes.centres;
    for (i, p) in all.iter().enumerate() {
        for q in &all[i + 1..] {
            if p.y == q.y {
                assert!(
                    (p.x - q.x).abs() >= 34,
                    "column pitch below 340px: {p:?} {q:?}"
                );
            }
        }
    }
}
