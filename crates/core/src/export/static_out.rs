//! c4rizer's own static site (P3-423, P3-424, ruling E4): `index.html`,
//! one `<key>.svg` and one `view-<key>.html` per drawable view. Plain
//! HTML and SVG: no script, no network, no vendored JS.

// DECISION: view pages are `view-<key>.html`, not `<key>.html`: a view
// keyed `index` would otherwise overwrite the index, and the prefix
// keeps every page name apart from `index.html` for every key.
// DECISION: the EXP-1 scan of a page (P3-423) is: no `<script`, no event
// attribute, no `javascript:`, no `url(`, no `@import`, no URI other than
// the SVG namespace, and every `href`/`src` names a file of the site.
// Links between the pages are what P3-424 asks for, so a relative `href`
// is allowed in the HTML; the SVGs stay the `name` mode files of EXP-1.
// DECISION: a view page shows its SVG with `<img>` at the SVG's own size
// and links the enterable boxes with an image map (`<map>`/`<area>`), so
// the links work without script; the same links are listed under the
// picture for keyboard and screen-reader use.
// DECISION: a box links only to a view of the site: the first declared
// view of the counting kind scoped to its element (a container view for
// a software system, a component view for a container), else an image
// view scoped to it, as the editor's `view_into` picks (EDIT-15). An
// EDIT-15 default view is not written to the site, so a box whose only
// view is a default view links nowhere.
// DECISION: the index lists every view of the file, unsupported ones
// as plain text with no link; filtered views' bases are listed too
// (P3-424 "every view"), in the view list order of EDIT-8 and P2-716
// (`structurizr.sort` `key` or `type`, else declaration order).

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::model::{ElementId, ElementKind, Model, ViewKind, ViewSpec};
use crate::pipeline::AllViews;

/// The files of the site, paths relative to the output folder: the
/// index first, then per drawable view in view list order its SVG and
/// its page.
pub(super) fn site(model: &Model, all: &AllViews) -> Vec<(PathBuf, Vec<u8>)> {
    let order = view_order(model);
    let mut files = vec![(PathBuf::from("index.html"), index(model, all, &order).into_bytes())];
    for spec in &order {
        let Some(view) = all.view(&spec.key) else { continue };
        let key = &view.plan.key;
        files.push((PathBuf::from(format!("{key}.svg")), super::svg(&view.scene).into_bytes()));
        files.push((PathBuf::from(page_name(key)), page(model, all, spec, view).into_bytes()));
    }
    files
}

/// The page file of view `key`.
fn page_name(key: &str) -> String {
    format!("view-{key}.html")
}

/// The views of the file in view list order (EDIT-8, P2-716).
fn view_order(model: &Model) -> Vec<&ViewSpec> {
    let mut list: Vec<&ViewSpec> = model.views.iter().collect();
    let sort = model
        .views_properties
        .iter()
        .rev()
        .find(|p| p.name.value == "structurizr.sort")
        .map(|p| p.value.value.to_ascii_lowercase());
    let rank = |kind: ViewKind| match kind {
        ViewKind::SystemLandscape => 0,
        ViewKind::SystemContext => 1,
        ViewKind::Container => 2,
        ViewKind::Component => 3,
        ViewKind::Filtered => 4,
        ViewKind::Custom => 5,
        ViewKind::Image => 6,
        ViewKind::Dynamic => 7,
        ViewKind::Deployment => 8,
    };
    match sort.as_deref() {
        Some("key") => list.sort_by(|a, b| a.key.cmp(&b.key)),
        Some("type") => list.sort_by(|a, b| (rank(a.kind), &a.key).cmp(&(rank(b.kind), &b.key))),
        _ => {}
    }
    list
}

/// The drawn title of `spec`, else its key.
fn title(spec: &ViewSpec) -> &str {
    spec.drawn_title().map_or(spec.key.as_str(), |t| t.value.as_str())
}

/// The workspace name, else `Workspace`.
fn workspace_name(model: &Model) -> &str {
    model.name.as_ref().map_or("Workspace", |n| n.value.as_str())
}

/// The page head and the opening of the body.
fn head(out: &mut String, page_title: &str) {
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(out, "<title>{}</title>", escape(page_title));
    out.push_str(STYLE);
    out.push_str("</head>\n<body>\n");
}

/// The page style: plain CSS, no `url(`, no `@import`.
const STYLE: &str = "<style>\n\
body { font-family: Inter, sans-serif; margin: 24px 16px; color: #1a1a1a; background: #ffffff; }\n\
h1 { font-size: 1.5em; margin: 0 0 8px; }\n\
p.description, p.kind { color: #555555; margin: 0 0 16px; }\n\
ol.views li, ul.opens li { margin: 4px 0; }\n\
code { color: #555555; }\n\
div.picture { overflow: auto; border: 1px solid #dddddd; margin: 16px 0; }\n\
div.picture img { display: block; max-width: none; }\n\
</style>\n";

/// `index.html`: every view in view list order with its title and key
/// (P3-424).
fn index(model: &Model, all: &AllViews, order: &[&ViewSpec]) -> String {
    let mut out = String::new();
    let name = workspace_name(model);
    head(&mut out, name);
    let _ = writeln!(out, "<h1>{}</h1>", escape(name));
    if let Some(d) = &model.description {
        let _ = writeln!(out, "<p class=\"description\">{}</p>", escape(&d.value));
    }
    out.push_str("<ol class=\"views\">\n");
    for spec in order {
        let key = escape(&spec.key);
        let kind = spec.kind.keyword();
        if all.view(&spec.key).is_some() {
            let _ = writeln!(
                out,
                "<li><a href=\"{}\">{}</a> <code>{key}</code> <span class=\"kind\">{kind}</span></li>",
                href(&page_name(&spec.key)),
                escape(title(spec)),
            );
        } else {
            let _ = writeln!(
                out,
                "<li>{} <code>{key}</code> <span class=\"kind\">{kind}, not drawn</span></li>",
                escape(title(spec)),
            );
        }
    }
    out.push_str("</ol>\n</body>\n</html>\n");
    out
}

/// The page of one drawn view: back link, title, the SVG with an image
/// map over its enterable boxes, and the same links as a list (P3-424).
fn page(model: &Model, all: &AllViews, spec: &ViewSpec, view: &crate::pipeline::ViewBuild) -> String {
    let mut out = String::new();
    let scene = &view.scene;
    let key = &view.plan.key;
    head(&mut out, &format!("{} - {}", title(spec), workspace_name(model)));
    out.push_str("<p><a href=\"index.html\">All views</a></p>\n");
    let _ = writeln!(out, "<h1>{}</h1>", escape(title(spec)));
    let _ = writeln!(out, "<p class=\"kind\"><code>{}</code> {}</p>", escape(key), spec.kind.keyword());
    let links: Vec<(String, String, String)> = scene
        .boxes
        .iter()
        .filter_map(|b| {
            let target = opens(model, all, b.element).filter(|t| t.key != *key)?;
            let r = b.rect;
            let (x0, y0) = (r.min.x - scene.bounds.min.x, r.min.y - scene.bounds.min.y);
            let coords = format!("{x0},{y0},{},{}", x0 + r.width(), y0 + r.height());
            Some((coords, model.element(b.element).name.value.clone(), target.key.clone()))
        })
        .collect();
    let map = if links.is_empty() { "" } else { " usemap=\"#boxes\"" };
    let _ = writeln!(
        out,
        "<div class=\"picture\"><img src=\"{}\" width=\"{}\" height=\"{}\" alt=\"{}\"{map}></div>",
        href(&format!("{key}.svg")),
        scene.bounds.width(),
        scene.bounds.height(),
        escape(title(spec)),
    );
    if !links.is_empty() {
        out.push_str("<map name=\"boxes\">\n");
        for (coords, name, target) in &links {
            let _ = writeln!(
                out,
                "<area shape=\"rect\" coords=\"{coords}\" href=\"{}\" alt=\"{}\">",
                href(&page_name(target)),
                escape(name),
            );
        }
        out.push_str("</map>\n<ul class=\"opens\">\n");
        for (_, name, target) in &links {
            let target_title = model.view(target).map_or(target.as_str(), title);
            let _ = writeln!(
                out,
                "<li>{} opens <a href=\"{}\">{}</a></li>",
                escape(name),
                href(&page_name(target)),
                escape(target_title),
            );
        }
        out.push_str("</ul>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

/// The drawn view of the site a click on `element` opens (header
/// DECISION, EDIT-15), `None` when it opens none.
fn opens<'a>(model: &'a Model, all: &AllViews, element: ElementId) -> Option<&'a ViewSpec> {
    let counting = match model.element(element).kind {
        ElementKind::SoftwareSystem => Some(ViewKind::Container),
        ElementKind::Container => Some(ViewKind::Component),
        _ => None,
    };
    let scoped = |kind: Option<ViewKind>| {
        model
            .views
            .iter()
            .find(|v| Some(v.kind) == kind && v.scope.element() == Some(element))
    };
    scoped(counting)
        .or_else(|| scoped(Some(ViewKind::Image)))
        .filter(|v| all.view(&v.key).is_some())
}

/// `text` with `&`, `<`, `>`, `"` and `'` escaped for HTML text and
/// attribute values.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A file name of the site as an `href`/`src` value: every byte outside
/// the URL's unreserved set percent-encoded, so a key with a space, `#`
/// or `?` still names its file.
fn href(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{escape, href};

    #[test]
    fn href_encodes_reserved_bytes() {
        assert_eq!(href("view-a b#c.html"), "view-a%20b%23c.html");
        assert_eq!(href("view-é.html"), "view-%C3%A9.html");
        assert_eq!(escape("<a & 'b'>\""), "&lt;a &amp; &#39;b&#39;&gt;&quot;");
    }
}
