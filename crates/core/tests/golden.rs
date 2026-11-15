//! Golden check 1 (CLI-5, CORE-11) on `tests/data/example.dsl`:
//! `pipeline::build_all` compared structurally with the prototype SVGs in
//! `tests/data/example/*.svg` (pipeline-02). Golden check 2 (byte-identical
//! against `tests/data/golden/`) is the CLI's `cli_5_example_golden`.
//!
//! The reference SVGs are the prototype's output (`c4.py` on `example/`,
//! copied unchanged). They carry no ids, so the structure is read back
//! from the drawing: a box is a shape followed by its bold 15px name; a
//! line is a round-joined 2px path, joined when it has no arrowhead (it
//! ends in a dot on its trunk); a label is a white 0.93-opaque rectangle
//! followed by its 11px text lines.

// DECISION: boxes are compared by name (the reference has no ids; every
// name in the example is unique per view), lines by (source name,
// destination name), labels as the multiset of their texts with lines
// joined by one space (wrapping widths differ between the prototype's
// font metrics and ours; the words must not).
// DECISION: "same joined lines" (CLI-5) is compared per destination box:
// how many lines end in a join dot on each box. Which lines merge is the
// router's choice within CORE-8 (prototype A* vs ours): on C3-serving
// both join one line into `auth` and one into `ledger`, but the prototype
// merges `notify -> ledger` with `sync -> ledger` where ours merges it
// with `web -> ledger`, and it keeps `web -> auth` as trunk where ours
// keeps `store -> auth` (docs/deferred.md observation row).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::parse;
use c4rizer_core::pipeline::{ViewBuild, build_all};

const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/data/example.dsl"
));

/// (view key, reference SVG, the reference's routed crossings per SRS
/// CORE-11).
const REFERENCES: [(&str, &str, u32); 4] = [
    (
        "C1",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/example/c1.svg"
        )),
        0,
    ),
    (
        "C2",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/example/c2.svg"
        )),
        0,
    ),
    (
        "C3-serving",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/example/c3.svg"
        )),
        1,
    ),
    (
        "C3-startup",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/example/c3-startup.svg"
        )),
        0,
    ),
];

/// A view's structure, from either side.
#[derive(Debug, Default, PartialEq, Eq)]
struct Structure {
    boxes: BTreeSet<String>,
    lines: BTreeSet<(String, String)>,
    /// Destination box name -> lines ending in a join dot on it.
    joined: BTreeMap<String, usize>,
    labels: BTreeMap<String, usize>,
}

/// Axis-aligned rectangle in px: (x0, y0, x1, y1).
type Px = (f64, f64, f64, f64);

/// The value of attribute `name` in the first tag of `s` that has it.
fn attr<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = s.find(&key)? + key.len();
    let len = s[start..].find('"')?;
    Some(&s[start..start + len])
}

fn num(s: &str) -> f64 {
    s.trim()
        .parse()
        .unwrap_or_else(|_| panic!("not a number: {s:?}"))
}

fn unescape(s: &str) -> String {
    s.replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The content of the first `<text>` element of `s` whose tag holds
/// `marker`.
fn text_with(s: &str, marker: &str) -> Option<String> {
    let mut rest = s;
    while let Some(i) = rest.find("<text") {
        let tag_end = i + rest[i..].find('>')?;
        let end = tag_end + rest[tag_end..].find("</text>")?;
        if rest[i..tag_end].contains(marker) {
            return Some(unescape(&rest[tag_end + 1..end]));
        }
        rest = &rest[end..];
    }
    None
}

/// The outline of a box line of the reference: rect (rx 6), person
/// (rect rx 22 starting 50px below the top, head circle above) or
/// cylinder (`M x,y+ry a w/2,ry 0 0 1 w,0 v h-2ry ...`).
fn box_rect(line: &str) -> Px {
    if line.starts_with("<rect") {
        let (x, y) = (num(attr(line, "x").unwrap()), num(attr(line, "y").unwrap()));
        let w = num(attr(line, "width").unwrap());
        let h = num(attr(line, "height").unwrap());
        if attr(line, "rx") == Some("22") {
            return (x, y - 50.0, x + w, y + h);
        }
        return (x, y, x + w, y + h);
    }
    let d = attr(line, "d").expect("cylinder path");
    let parts: Vec<&str> = d
        .split(|c: char| c.is_ascii_alphabetic() || c == ' ' || c == ',')
        .filter(|p| !p.is_empty())
        .collect();
    // M x y a rx ry rot large sweep dx dy v dv ...
    let (x, top_ry, ry, w, v) = (
        num(parts[0]),
        num(parts[1]),
        num(parts[3]),
        num(parts[7]),
        num(parts[9]),
    );
    (x, top_ry - ry, x + w, top_ry - ry + v + 2.0 * ry)
}

fn points(d: &str) -> Vec<(f64, f64)> {
    d.split(['M', 'L'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (x, y) = p.split_once(',').expect("x,y");
            (num(x), num(y))
        })
        .collect()
}

fn on_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    (p.0 - qx).hypot(p.1 - qy) <= 1.0
}

/// The box whose outline, grown by 3px, holds `p`.
fn box_at(boxes: &[(String, Px)], p: (f64, f64)) -> Option<usize> {
    boxes.iter().position(|(_, r)| {
        p.0 >= r.0 - 3.0 && p.0 <= r.2 + 3.0 && p.1 >= r.1 - 3.0 && p.1 <= r.3 + 3.0
    })
}

/// The structure of a reference SVG.
fn reference(svg: &str) -> Structure {
    let mut boxes: Vec<(String, Px)> = Vec::new();
    let mut paths: Vec<(Vec<(f64, f64)>, bool)> = Vec::new();
    let mut labels = BTreeMap::new();
    let lines: Vec<&str> = svg.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(name) = text_with(line, "font-size=\"15\"") {
            boxes.push((name, box_rect(line)));
        } else if line.starts_with("<path") && line.contains("stroke-linejoin=\"round\"") {
            let joined = !line.contains("marker-end");
            paths.push((points(attr(line, "d").expect("d")), joined));
        } else if line.starts_with("<rect") && line.contains("fill-opacity=\"0.93\"") {
            let mut words = Vec::new();
            while let Some(t) = lines
                .get(i + 1)
                .and_then(|l| text_with(l, "font-size=\"11\""))
            {
                words.push(t);
                i += 1;
            }
            *labels.entry(words.join(" ")).or_insert(0) += 1;
        }
        i += 1;
    }
    let name = |p: (f64, f64)| -> usize {
        box_at(&boxes, p).unwrap_or_else(|| panic!("no reference box at {p:?}"))
    };
    // The path a joined path `k` ends on: the one its dot sits on.
    let trunk_of = |k: usize| -> Option<usize> {
        let (pts, joined) = &paths[k];
        if !joined {
            return None;
        }
        let end = *pts.last().expect("points");
        let trunk = paths
            .iter()
            .enumerate()
            .find(|(j, (q, _))| *j != k && q.windows(2).any(|w| on_segment(end, w[0], w[1])))
            .map_or_else(|| panic!("joined line ends on no trunk at {end:?}"), |(j, _)| j);
        Some(trunk)
    };
    // The unjoined path at the root of `k`'s join chain.
    let root = |k: usize| -> usize {
        let mut at = k;
        for _ in 0..=paths.len() {
            match trunk_of(at) {
                Some(t) => at = t,
                None => return at,
            }
        }
        panic!("join cycle")
    };
    let mut out = Structure {
        boxes: boxes.iter().map(|(n, _)| n.clone()).collect(),
        labels,
        ..Structure::default()
    };
    assert_eq!(out.boxes.len(), boxes.len(), "box names are unique");
    for (k, (pts, joined)) in paths.iter().enumerate() {
        let end = *paths[root(k)].0.last().expect("points");
        let src = boxes[name(pts[0])].0.clone();
        let dst = boxes[name(end)].0.clone();
        if *joined {
            *out.joined.entry(dst.clone()).or_insert(0) += 1;
        }
        out.lines.insert((src, dst));
    }
    out
}

/// The structure of one of our builds.
fn ours(b: &ViewBuild) -> Structure {
    let name = |id: c4rizer_core::view::BoxId| b.plan.boxes[id.0 as usize].name.clone();
    let mut out = Structure {
        boxes: b.plan.boxes.iter().map(|x| x.name.clone()).collect(),
        ..Structure::default()
    };
    for l in &b.scene.lines {
        let pair = (name(l.source), name(l.destination));
        if l.joins.is_some() {
            *out.joined.entry(pair.1.clone()).or_insert(0) += 1;
        }
        out.lines.insert(pair);
    }
    for l in &b.scene.labels {
        *out.labels.entry(l.text.lines.join(" ")).or_insert(0) += 1;
    }
    out
}

/// CLI-5 (1), CORE-11: every example view has the reference's box ids,
/// the same edge pairs, the same joined lines, a label wherever the
/// reference has one, and crossings no higher than the reference.
#[test]
fn golden_example_structure_matches_reference() {
    let model = match parse(EXAMPLE) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    };
    let all = build_all(&model, &AtomicBool::new(false)).expect("never stopped");
    assert_eq!(all.views.len(), REFERENCES.len());
    for (key, svg, crossings) in REFERENCES {
        let b = all.view(key).unwrap_or_else(|| panic!("no view {key}"));
        let want = reference(svg);
        let got = ours(b);
        assert!(!want.boxes.is_empty() && !want.lines.is_empty(), "{key}");
        assert_eq!(got.boxes, want.boxes, "{key}: boxes");
        assert_eq!(got.lines, want.lines, "{key}: lines");
        assert_eq!(got.joined, want.joined, "{key}: join dots per box");
        assert_eq!(got.labels, want.labels, "{key}: labels");
        assert!(b.scene.unroutable().is_empty(), "{key}: unroutable");
        assert!(
            b.scene.crossings <= crossings,
            "{key}: {} crossings, reference {crossings}",
            b.scene.crossings
        );
    }
}
