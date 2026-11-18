//! P2-25 SVG text options on `tests/data/example.dsl` (cli-breadth tasks
//! 09 and 10): `export::svg` unchanged, `export::svg_embedded` and, with
//! feature `outline`, `export::svg_outlined`.

use std::sync::atomic::AtomicBool;

use c4rizer_core::export;
use c4rizer_core::label::Weight;
use c4rizer_core::model::parse;
use c4rizer_core::pipeline::build_all;
use c4rizer_core::scene::{FONT_BOLD, FONT_FAMILY, FONT_REGULAR, Scene, TextBlock};

const EXAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/data/example.dsl"
));
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/data/golden");
const KEYS: [&str; 4] = ["C1", "C2", "C3-serving", "C3-startup"];

/// Every scene of the example, in `KEYS` order.
fn scenes() -> Vec<Scene> {
    let model = match parse(EXAMPLE) {
        Ok(p) => p.model,
        Err(e) => panic!("{e}"),
    };
    let Ok(all) = build_all(&model, &AtomicBool::new(false)) else {
        panic!("never stopped");
    };
    KEYS.iter()
        .map(|k| all.view(k).expect("view").scene.clone())
        .collect()
}

/// Every text block of `scene`, mutable.
fn texts(scene: &mut Scene) -> Vec<&mut TextBlock> {
    let mut out: Vec<&mut TextBlock> = Vec::new();
    out.extend(scene.boundaries.iter_mut().map(|b| &mut b.tag));
    for b in &mut scene.boxes {
        out.push(&mut b.name);
        out.push(&mut b.type_line);
        out.extend(b.description.iter_mut());
    }
    out.extend(scene.labels.iter_mut().map(|l| &mut l.text));
    out.extend(scene.title.iter_mut());
    out.extend(scene.subtitle.iter_mut());
    out.extend(scene.legend.entries.iter_mut().map(|e| &mut e.text));
    out
}

/// `scene` with every text in `weight`.
fn all_in(mut scene: Scene, weight: Weight) -> Scene {
    for t in texts(&mut scene) {
        t.weight = weight;
    }
    scene
}

/// The EXP-1 no-script, no-external-reference scan (the private scan of
/// `export.rs` as text checks). `font_data` allows exactly the
/// `url(data:font/ttf;base64,...)` sources of `@font-face` rules
/// (P2-657); everything else is as strict as for `name` (P2-659).
fn exp_1_scan(svg: &str, font_data: bool) -> Result<(), String> {
    let mut text = svg.to_owned();
    if font_data {
        // Cut every allowed font source out, then scan what is left.
        while let Some(start) = text.find("src:url(data:font/ttf;base64,") {
            let end = text[start..].find(')').ok_or("unclosed url(")? + start;
            let payload = &text[start + "src:url(data:font/ttf;base64,".len()..end];
            if !payload
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
            {
                return Err("font data is not plain base64".into());
            }
            text.replace_range(start..=end, "src:FONT");
        }
    }
    for banned in [
        "<script",
        "<image",
        "<foreignObject",
        "<use",
        "url(",
        "@import",
        "xlink:",
        "href",
        "data:",
        "javascript:",
    ] {
        if text.contains(banned) {
            return Err(format!("{banned} found"));
        }
    }
    if !font_data && text.contains("@font-face") {
        return Err("@font-face found".into());
    }
    // An event attribute: ` on<letters>=`.
    let lower = text.to_ascii_lowercase();
    for (i, _) in lower.match_indices(" on") {
        let rest = &lower[i + 3..];
        let word = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        if word > 0 && rest[word..].starts_with('=') {
            return Err("event attribute found".into());
        }
    }
    if text.matches("://").count() != 1 || !text.contains("xmlns=\"http://www.w3.org/2000/svg\"") {
        return Err("a URI other than the SVG namespace".into());
    }
    Ok(())
}

/// The `@font-face` rules of an embedded SVG: (weight, decoded bytes).
fn font_faces(svg: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find("@font-face{") {
        let rule_end = rest[i..].find('}').expect("rule closed") + i;
        let rule = &rest[i..rule_end];
        assert!(
            rule.contains(&format!("font-family:\"{FONT_FAMILY}\"")),
            "{}",
            &rule[..rule.len().min(120)]
        );
        let weight = rule
            .split("font-weight:")
            .nth(1)
            .and_then(|r| r.split(';').next())
            .expect("font-weight")
            .to_owned();
        let data = rule
            .split("src:url(data:font/ttf;base64,")
            .nth(1)
            .and_then(|r| r.split(')').next())
            .expect("src");
        out.push((weight, base64_decode(data)));
        rest = &rest[rule_end..];
    }
    out
}

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => u32::from(c - b'A'),
        b'a'..=b'z' => u32::from(c - b'a') + 26,
        b'0'..=b'9' => u32::from(c - b'0') + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let bytes = text.as_bytes();
    assert_eq!(bytes.len() % 4, 0, "padded base64");
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|b| **b == b'=').count();
        let n = chunk
            .iter()
            .take(4 - pad)
            .enumerate()
            .fold(0u32, |acc, (i, &c)| acc | value(c) << (18 - 6 * i));
        for i in 0..3 - pad {
            out.push(u8::try_from((n >> (16 - 8 * i)) & 0xff).unwrap());
        }
    }
    out
}

#[test]
fn p2_652_svg_default_unchanged() {
    for (key, scene) in KEYS.iter().zip(scenes()) {
        let golden = std::fs::read_to_string(format!("{GOLDEN}/{key}.svg")).expect("golden");
        assert!(export::svg(&scene) == golden, "{key}: svg differs from its CLI-5 golden");
    }
}

#[test]
fn p2_654_named_svg_still_passes_exp_1() {
    for (key, scene) in KEYS.iter().zip(scenes()) {
        let svg = export::svg(&scene);
        exp_1_scan(&svg, false).unwrap_or_else(|e| panic!("{key}: {e}"));
        assert!(!svg.contains("<style"), "{key}");
        assert!(svg.contains(&format!("font-family=\"{FONT_FAMILY}, sans-serif\"")), "{key}");
    }
}

#[test]
fn p2_655_embed_one_font_face_per_used_face_with_weight() {
    let scene = scenes().swap_remove(0);
    let both = export::svg_embedded(&scene);
    assert_eq!(both.matches("<style>").count(), 1);
    let weights: Vec<String> = font_faces(&both).into_iter().map(|f| f.0).collect();
    assert_eq!(weights, ["normal", "bold"]);
    let regular = export::svg_embedded(&all_in(scene.clone(), Weight::Regular));
    let weights: Vec<String> = font_faces(&regular).into_iter().map(|f| f.0).collect();
    assert_eq!(weights, ["normal"], "a scene with no bold text has one @font-face");
    let bold = export::svg_embedded(&all_in(scene, Weight::Bold));
    let weights: Vec<String> = font_faces(&bold).into_iter().map(|f| f.0).collect();
    assert_eq!(weights, ["bold"]);
}

#[test]
fn p2_656_embed_font_face_decodes_to_bundled_bytes() {
    let scene = scenes().swap_remove(0);
    let faces = font_faces(&export::svg_embedded(&scene));
    assert_eq!(faces.len(), 2);
    for (weight, bytes) in faces {
        let want = if weight == "bold" { FONT_BOLD } else { FONT_REGULAR };
        assert!(bytes == want, "{weight}: decoded bytes differ from the bundled face");
    }
}

#[test]
fn p2_657_embed_no_external_reference_but_font_data() {
    for (key, scene) in KEYS.iter().zip(scenes()) {
        let embedded = export::svg_embedded(&scene);
        exp_1_scan(&embedded, true).unwrap_or_else(|e| panic!("{key}: {e}"));
        assert!(exp_1_scan(&embedded, false).is_err(), "{key}: the font data is there");
        // Apart from the style, the drawing is the named SVG's.
        let style_start = embedded.find("<style>").expect("style");
        let style_end = embedded.find("</style>\n").expect("style end") + "</style>\n".len();
        let mut rest = embedded.clone();
        rest.replace_range(style_start..style_end, "");
        assert!(rest == export::svg(&scene), "{key}");
    }
}

#[cfg(feature = "outline")]
#[test]
fn p2_658_outline_has_no_text_or_font() {
    for (key, scene) in KEYS.iter().zip(scenes()) {
        let named = export::svg(&scene);
        let outlined = export::svg_outlined(&scene).expect("outlined");
        for banned in ["<text", "font-family", "@font-face", "<style"] {
            assert!(!outlined.contains(banned), "{key}: {banned}");
        }
        assert!(outlined.contains("<path"), "{key}");
        // Every text colour of the named SVG fills some outline path.
        let fills: std::collections::BTreeSet<&str> = named
            .split("<text ")
            .skip(1)
            .filter_map(|t| t.split(" fill=\"").nth(1))
            .filter_map(|r| r.split('"').next())
            .collect();
        assert!(!fills.is_empty(), "{key}");
        for fill in fills {
            assert!(
                outlined.contains(&format!("fill=\"{fill}\"")),
                "{key}: no path filled {fill}"
            );
        }
        assert_eq!(export::svg_outlined(&scene).expect("again"), outlined, "{key}: deterministic");
    }
}

#[cfg(feature = "outline")]
#[test]
fn p2_659_outline_passes_exp_1_scan() {
    for (key, scene) in KEYS.iter().zip(scenes()) {
        let outlined = export::svg_outlined(&scene).expect("outlined");
        exp_1_scan(&outlined, false).unwrap_or_else(|e| panic!("{key}: {e}"));
    }
}

#[cfg(all(feature = "outline", feature = "png"))]
#[test]
fn p2_660_outline_raster_matches_named_raster() {
    use resvg::{tiny_skia, usvg};

    /// `svg` rasterized at its own size with `fonts` loaded (no others).
    fn raster(svg: &str, fonts: &[&[u8]]) -> tiny_skia::Pixmap {
        let mut db = usvg::fontdb::Database::new();
        for f in fonts {
            db.load_font_data(f.to_vec());
        }
        if !fonts.is_empty() {
            db.set_sans_serif_family(FONT_FAMILY);
        }
        let opt = usvg::Options {
            font_family: FONT_FAMILY.to_owned(),
            fontdb: std::sync::Arc::new(db),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_str(svg, &opt).expect("parses");
        let size = tree.size().to_int_size();
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).expect("pixmap");
        resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
        pixmap
    }

    // TODO(srs): P2-660, the allowed difference. Reading taken (B
    // cli-breadth 10): at most 0.5% of pixels differ, at 1x.
    // DECISION: a pixel differs when any channel differs at all.
    for (key, scene) in KEYS.iter().zip(scenes()) {
        let named = raster(&export::svg(&scene), &[FONT_REGULAR, FONT_BOLD]);
        let outlined = raster(&export::svg_outlined(&scene).expect("outlined"), &[]);
        assert_eq!(
            (named.width(), named.height()),
            (outlined.width(), outlined.height()),
            "{key}"
        );
        let differ = named
            .pixels()
            .iter()
            .zip(outlined.pixels())
            .filter(|(a, b)| a != b)
            .count();
        let total = named.pixels().len();
        assert!(
            differ * 200 <= total,
            "{key}: {differ} of {total} pixels differ (over 0.5%)"
        );
    }
}

/// EXP-1 as P2-409 relaxes it for icons: each `<image ... href="data:image/<type>;base64,...">`
/// is cut out (its payload must be plain base64), then `exp_1_scan` runs
/// on what is left with no other exception.
// DECISION: P2-659's "no exception" read as "the scan the `name` export of
// the same view passes"; for a view with an icon that scan carries P2-409's
// `<image>` data-URI allowance (no external reference, SRS EXP-1 text).
#[cfg(feature = "outline")]
fn exp_1_scan_icons(svg: &str) -> Result<(), String> {
    let mut text = svg.to_owned();
    while let Some(start) = text.find("<image ") {
        let end = text[start..].find('>').ok_or("unclosed <image")? + start;
        let tag = &text[start..=end];
        let at = tag.find(" href=\"data:image/").ok_or_else(|| format!("image without data href: {tag:.80}"))?;
        let value = &tag[at + " href=\"".len()..];
        let value = &value[..value.find('"').ok_or("unclosed href")?];
        let (_, payload) = value.split_once(";base64,").ok_or("not base64")?;
        if !payload
            .trim_start()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        {
            return Err("icon data is not plain base64".into());
        }
        text.replace_range(start..=end, "<ICON/>");
    }
    exp_1_scan(&text, false)
}

/// P2-659 with an icon (P2-409): the outlined SVG passes the same EXP-1
/// scan as the `name` SVG of that view (`<image href="data:...">`, no
/// `xlink:`, one URI).
#[cfg(feature = "outline")]
#[test]
fn p2_659_outline_with_icon_passes_exp_1_scan() {
    let src = r#"workspace {
  model {
    operator = person "Operator" "Runs the shop."
    shop = softwareSystem "Shop" "Sells goods."
    operator -> shop "Uses" "HTTPS"
  }
  views {
    systemContext shop "ctx" {
      include *
    }
    styles {
      element "Person" {
        icon "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxMCIgaGVpZ2h0PSIxMCI+PHJlY3Qgd2lkdGg9IjEwIiBoZWlnaHQ9IjEwIiBmaWxsPSJyZWQiLz48L3N2Zz4="
      }
    }
  }
}
"#;
    let model = parse(src).expect("parses").model;
    let Ok(all) = build_all(&model, &AtomicBool::new(false)) else {
        panic!("never stopped");
    };
    let scene = &all.view("ctx").expect("view").scene;
    exp_1_scan_icons(&export::svg(scene)).unwrap_or_else(|e| panic!("name: {e}"));
    let outlined = export::svg_outlined(scene).expect("outlined");
    assert!(outlined.contains("<image "), "the icon is kept");
    exp_1_scan_icons(&outlined).unwrap_or_else(|e| panic!("outline: {e}"));
    // The plain `href` is still read as the icon: usvg reparses it into an
    // image node and writes it back.
    let reread = usvg::Tree::from_str(&outlined, &usvg::Options::default()).expect("reparses");
    let rewritten = reread.to_string(&usvg::WriteOptions::default());
    assert!(rewritten.contains("data:image/svg+xml;base64,"), "icon lost on reparse");
}
