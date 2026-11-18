//! The workspace's own styles as a Structurizr theme,
//! `workspace-theme.json` (P3-421, P3-422).

// DECISION: blocks for one tag and colour scheme merge into one entry,
// later properties winning (the reference merges the two `S6` blocks);
// entries without a colour scheme come first, then `Dark`, then `Light`,
// each sorted by tag (reference `styles/all_styles-theme.json`).
// DECISION: c4rizer stores `dashed true|false` and `style ...` as one
// property (P2-420); the token's length tells them apart (`true` is 4
// bytes, every `style` value but `solid` 6), so `dashed true` comes
// back as `"dashed"`. `solid`/`false` (5 bytes) are written as `style`:
// `dashed false` should be `"dashed" : false` (docs/deferred.md).
// DECISION: an icon read from a file is written as a `data:` URI of its
// bytes, as Structurizr inlines it; a written `data:` URI or URL as is.
// DECISION: an empty `elements` or `relationships` array is left out.

use crate::model::{Border, ElementStyle, Mime, Mode, Model, Property, RelationshipStyle, Routing};

use super::json_out::{Obj, Value, print};
use super::structurizr::base64;

pub(super) fn write(model: &Model) -> String {
    let mut root = Obj::default();
    if let Some(n) = &model.name {
        root.put("name", Value::str(&n.value));
    }
    if let Some(d) = &model.description {
        root.put("description", Value::str(&d.value));
    }
    let elements = element_styles(model);
    if !elements.is_empty() {
        root.put("elements", Value::Arr(elements));
    }
    let relationships = relationship_styles(model);
    if !relationships.is_empty() {
        root.put("relationships", Value::Arr(relationships));
    }
    print(&root.value())
}

/// The workspace's own element styles, one entry per tag and scheme.
pub(super) fn element_styles(model: &Model) -> Vec<Value> {
    groups(&model.element_styles, |s| (s.tag.value.as_str(), s.mode, s.theme))
        .into_iter()
        .map(|(tag, mode, blocks)| element(model, tag, mode, &blocks))
        .collect()
}

/// The workspace's own relationship styles, one entry per tag and scheme.
pub(super) fn relationship_styles(model: &Model) -> Vec<Value> {
    groups(&model.relationship_styles, |s| (s.tag.value.as_str(), s.mode, s.theme))
        .into_iter()
        .map(|(tag, mode, blocks)| relationship(tag, mode, &blocks))
        .collect()
}

/// The workspace's own blocks (not those from themes) grouped by tag and
/// mode, in theme order.
fn groups<'s, T>(
    blocks: &'s [T],
    key: impl Fn(&'s T) -> (&'s str, Option<Mode>, Option<usize>),
) -> Vec<(&'s str, Option<Mode>, Vec<&'s T>)> {
    let mut out: Vec<(&str, Option<Mode>, Vec<&T>)> = Vec::new();
    for b in blocks {
        let (tag, mode, theme) = key(b);
        if theme.is_some() {
            continue;
        }
        match out.iter_mut().find(|(t, m, _)| *t == tag && *m == mode) {
            Some((_, _, list)) => list.push(b),
            None => out.push((tag, mode, vec![b])),
        }
    }
    let rank = |m: Option<Mode>| match m {
        None => 0,
        Some(Mode::Dark) => 1,
        Some(Mode::Light) => 2,
    };
    out.sort_by(|a, b| rank(a.1).cmp(&rank(b.1)).then_with(|| a.0.cmp(b.0)));
    out
}

fn scheme(mode: Option<Mode>) -> Option<Value> {
    mode.map(|m| Value::str(if m == Mode::Dark { "Dark" } else { "Light" }))
}

fn properties(list: &[Property]) -> Option<Value> {
    let mut o = Obj::default();
    for p in list {
        o.put(&p.name.value, Value::str(&p.value.value));
    }
    (!o.0.is_empty()).then(|| o.value())
}

fn border(b: Border) -> &'static str {
    match b {
        Border::Dashed => "Dashed",
        Border::Dotted => "Dotted",
        _ => "Solid",
    }
}

/// The last `Some` of `f` over `blocks`.
fn last<T, V>(blocks: &[&T], f: impl Fn(&T) -> Option<V>) -> Option<V> {
    blocks.iter().filter_map(|b| f(b)).last()
}

fn element(model: &Model, tag: &str, mode: Option<Mode>, blocks: &[&ElementStyle]) -> Value {
    let mut o = Obj::default();
    o.put("tag", Value::str(tag));
    o.opt("colorScheme", scheme(mode));
    let props: Vec<Property> = blocks.iter().flat_map(|b| b.properties.iter().cloned()).collect();
    o.opt("properties", properties(&props));
    o.opt("width", last(blocks, |b| b.width.map(|v| Value::int(v.value))));
    o.opt("height", last(blocks, |b| b.height.map(|v| Value::int(v.value))));
    let colour = |c: crate::model::Colour| Value::str(c.to_string());
    o.opt("background", last(blocks, |b| b.background.map(|v| colour(v.value))));
    o.opt("stroke", last(blocks, |b| b.stroke.map(|v| colour(v.value))));
    o.opt("strokeWidth", last(blocks, |b| b.stroke_width.map(|v| Value::int(v.value))));
    o.opt("color", last(blocks, |b| b.color.map(|v| colour(v.value))));
    o.opt("fontSize", last(blocks, |b| b.font_size.map(|v| Value::int(v.value))));
    o.opt("shape", last(blocks, |b| b.shape.map(|v| Value::str(v.value.keyword()))));
    o.opt("icon", last(blocks, |b| b.icon.as_ref().map(|i| Value::str(icon(model, &i.value)))));
    o.opt("border", last(blocks, |b| b.border.map(|v| Value::str(border(v.value)))));
    o.opt("opacity", last(blocks, |b| b.opacity.map(|v| Value::int(v.value))));
    o.opt("metadata", last(blocks, |b| b.metadata.map(|v| Value::Bool(v.value))));
    o.opt("description", last(blocks, |b| b.description.map(|v| Value::Bool(v.value))));
    o.value()
}

/// An icon as Structurizr writes it in a theme.
fn icon(model: &Model, written: &str) -> String {
    if written.starts_with("data:") {
        return written.to_owned();
    }
    match model.assets.iter().find(|a| a.path == written) {
        Some(a) => {
            let mime = match a.mime {
                Mime::Png => "image/png",
                Mime::Jpeg => "image/jpeg",
                _ => "image/svg+xml",
            };
            format!("data:{mime};base64,{}", base64(&a.bytes))
        }
        None => written.to_owned(),
    }
}

fn relationship(tag: &str, mode: Option<Mode>, blocks: &[&RelationshipStyle]) -> Value {
    let mut o = Obj::default();
    o.put("tag", Value::str(tag));
    o.opt("colorScheme", scheme(mode));
    let props: Vec<Property> = blocks.iter().flat_map(|b| b.properties.iter().cloned()).collect();
    o.opt("properties", properties(&props));
    o.opt("thickness", last(blocks, |b| b.thickness.map(|v| Value::int(v.value))));
    o.opt("color", last(blocks, |b| b.color.map(|v| Value::str(v.value.to_string()))));
    o.opt("fontSize", last(blocks, |b| b.font_size.map(|v| Value::int(v.value))));
    o.opt("width", last(blocks, |b| b.width.map(|v| Value::int(v.value))));
    if let Some(s) = last(blocks, |b| b.style) {
        if s.span.end - s.span.start == 4 && s.value == Border::Dashed {
            o.put("dashed", Value::Bool(true));
        } else {
            o.put("style", Value::str(border(s.value)));
        }
    }
    o.opt(
        "routing",
        last(blocks, |b| {
            b.routing.map(|v| {
                Value::str(match v.value {
                    Routing::Direct => "Direct",
                    Routing::Curved => "Curved",
                    _ => "Orthogonal",
                })
            })
        }),
    );
    o.opt("jump", last(blocks, |b| b.jump.map(|v| Value::Bool(v.value))));
    o.opt("position", last(blocks, |b| b.position.map(|v| Value::int(v.value))));
    o.opt("opacity", last(blocks, |b| b.opacity.map(|v| Value::int(v.value))));
    o.value()
}
