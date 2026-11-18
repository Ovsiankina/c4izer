//! `PlantUML`, as Structurizr 6.2.4's `StructurizrPlantUMLExporter` (with
//! its `-key.puml` legend) and `C4PlantUMLExporter` write it (P3-404,
//! P3-406, P3-407), read from `structurizr-export` 6.2.4's sources.

// DECISION: text is written as Structurizr writes it, quotes and angle
// brackets unescaped (ruling E1); a line break inside a text becomes `\n`
// (`structurizr::Writer`).
// DECISION: an icon is drawn only from an `http(s)` URL, as Structurizr
// does; its scale is Structurizr's fallback 0.5, since c4rizer never
// fetches the picture to measure it (Structurizr would).
// DECISION: the `plantuml.title`, `plantuml.skinparams`,
// `plantuml.includes`, `plantuml.sequenceDiagram`, `c4plantuml.tags`,
// `c4plantuml.legend` and `c4plantuml.stereotypes` properties keep their
// defaults (not read).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::model::{Border, Colour, ElementId, ElementKind, Shape, ViewKind};

use super::super::structurizr::{GREY, RelStyle, Style, TagStyle, View, ViewRel, WHITE, Workspace, Writer, base64};
use super::{Diagram, roles, separator, walk};

/// The Structurizr-flavour diagram and its legend; `groups` is the group
/// number Structurizr's exporter carries from view to view.
pub(super) fn structurizr(ws: &Workspace<'_>, view: &View<'_>, groups: &mut u32) -> (String, String) {
    let classes = Classes::of(ws, view);
    let mut d = Structurizr {
        classes: &classes,
        group: groups,
    };
    let diagram = walk(ws, view, &mut d).finish();
    (diagram, legend(&classes))
}

/// The C4-PlantUML diagram.
pub(super) fn c4(ws: &Workspace<'_>, view: &View<'_>) -> String {
    walk(ws, view, &mut C4 { group: 0 }).finish()
}

/// A box style of the style block (`PlantUMLElementStyle` and friends).
#[derive(Debug, Clone)]
struct BoxStyle {
    background: Colour,
    stroke: Colour,
    color: Colour,
    stroke_width: u8,
    border: Border,
    font_size: u32,
    shadow: bool,
    shape: Shape,
}

impl From<&Style> for BoxStyle {
    fn from(s: &Style) -> BoxStyle {
        BoxStyle {
            background: s.background,
            stroke: s.stroke,
            color: s.color,
            stroke_width: s.stroke_width_set.unwrap_or(2),
            border: s.border,
            font_size: s.font_size,
            shadow: s.shadow,
            shape: s.shape,
        }
    }
}

/// The style classes a view uses, each keyed by its name (sorted as
/// Structurizr sorts them); the first one added wins, as in its set.
#[derive(Debug, Default)]
struct Classes {
    elements: BTreeMap<String, Style>,
    relationships: BTreeMap<String, RelStyle>,
    nodes: BTreeMap<String, Style>,
    boundaries: BTreeMap<String, BoxStyle>,
    groups: BTreeMap<String, BoxStyle>,
}

impl Classes {
    fn of(ws: &Workspace<'_>, view: &View<'_>) -> Classes {
        let roles = roles(ws, view);
        let mut c = Classes::default();
        for &e in &roles.boxes {
            let style = ws.style(e);
            c.elements.entry(style.tags.join(",")).or_insert(style);
        }
        for r in &view.relationships {
            let s = ws.rel_style(r.rel);
            c.relationships.entry(s.tags.join(",")).or_insert(s);
        }
        for &e in &roles.nodes {
            let s = ws.style(e);
            c.nodes.entry(s.tags.join(",")).or_insert(s);
        }
        for &e in &roles.boundaries {
            c.boundaries
                .entry(ws.model.element(e).name.value.clone())
                .or_insert_with(|| boundary_style(ws, e));
        }
        for g in &roles.groups {
            c.groups.entry(g.clone()).or_insert_with(|| group_style(ws, g));
        }
        c
    }
}

/// The first of `one`, `all` that sets what `f` reads.
fn first<T>(one: Option<&TagStyle>, all: Option<&TagStyle>, f: impl Fn(&TagStyle) -> Option<T>) -> Option<T> {
    one.and_then(&f).or_else(|| all.and_then(&f))
}

/// `findBoundaryStyle`: `Boundary:<type>`, then `Boundary`, then the
/// element's own style.
fn boundary_style(ws: &Workspace<'_>, e: ElementId) -> BoxStyle {
    let kind = if ws.model.element(e).kind == ElementKind::SoftwareSystem { "SoftwareSystem" } else { "Container" };
    let one = ws.tag_style(&format!("Boundary:{kind}"));
    let all = ws.tag_style("Boundary");
    let (one, all) = (one.as_ref(), all.as_ref());
    let own = ws.style(e);
    BoxStyle {
        background: first(one, all, |s| s.background).unwrap_or(WHITE),
        stroke: first(one, all, |s| s.stroke).unwrap_or(own.stroke),
        color: first(one, all, |s| s.color).unwrap_or(own.color),
        stroke_width: first(one, all, |s| s.stroke_width).or(own.stroke_width_set).unwrap_or(2),
        border: first(one, all, |s| s.border).unwrap_or(own.border),
        font_size: first(one, all, |s| s.font_size).unwrap_or(own.font_size),
        shadow: own.shadow,
        shape: Shape::Box,
    }
}

/// `findGroupStyle`: `Group:<name>`, then `Group`, else Structurizr's
/// defaults (dotted).
fn group_style(ws: &Workspace<'_>, group: &str) -> BoxStyle {
    let one = ws.tag_style(&format!("Group:{group}"));
    let all = ws.tag_style("Group");
    let (one, all) = (one.as_ref(), all.as_ref());
    BoxStyle {
        background: first(one, all, |s| s.background).unwrap_or(WHITE),
        stroke: first(one, all, |s| s.stroke).unwrap_or(GREY),
        color: first(one, all, |s| s.color).unwrap_or(GREY),
        stroke_width: first(one, all, |s| s.stroke_width).unwrap_or(2),
        border: first(one, all, |s| s.border).unwrap_or(Border::Dotted),
        font_size: first(one, all, |s| s.font_size).unwrap_or(24),
        shadow: false,
        shape: Shape::Box,
    }
}

/// `PlantUML`'s `LineStyle` of a border or line `width` wide.
fn line_style(b: Border, width: u32) -> String {
    match b {
        Border::Dashed => format!("{}-{}", width * 5, width * 5),
        Border::Dotted => format!("{width}-{width}"),
        _ => "0".to_owned(),
    }
}

fn shape_keyword(s: Shape) -> &'static str {
    match s {
        Shape::Person | Shape::Robot => "person",
        Shape::Component => "component",
        Shape::Cylinder => "database",
        Shape::Folder => "folder",
        Shape::Ellipse | Shape::Circle => "storage",
        Shape::Hexagon => "hexagon",
        Shape::Pipe => "queue",
        _ => "rectangle",
    }
}

fn element_class(s: &Style) -> String {
    format!("Element-{}", base64(s.tags.join(",").as_bytes()))
}

fn rel_class(s: &RelStyle) -> String {
    format!("Relationship-{}", base64(s.tags.join(",").as_bytes()))
}

fn node_class(s: &Style) -> String {
    format!("DeploymentNode-{}", base64(s.tags.join(",").as_bytes()))
}

/// `calculateMetadataFontSize`: `floor(font * 0.7)`.
fn small(font: u32) -> u32 {
    font * 7 / 10
}

/// One box style's lines; `width` is its `MaximumWidth`, if any; an
/// element style (`element`) of a `RoundedBox` gets `RoundCorner`.
fn box_lines(w: &mut Writer, (name, class): (&str, &str), s: &BoxStyle, width: Option<i32>, element: bool) {
    w.line(&format!("  // {name}"));
    w.line(&format!("  .{class} {{"));
    w.line(&format!("    BackgroundColor: {};", s.background));
    w.line(&format!("    LineColor: {};", s.stroke));
    w.line(&format!("    LineStyle: {};", line_style(s.border, u32::from(s.stroke_width))));
    w.line(&format!("    LineThickness: {};", s.stroke_width));
    if element && s.shape == Shape::RoundedBox {
        w.line("    RoundCorner: 20;");
    }
    w.line(&format!("    FontColor: {};", s.color));
    w.line(&format!("    FontSize: {};", s.font_size));
    w.line("    HorizontalAlignment: center;");
    w.line(&format!("    Shadowing: {};", if s.shadow { 10 } else { 0 }));
    if let Some(width) = width {
        w.line(&format!("    MaximumWidth: {width};"));
    }
    w.line("  }");
}

/// The `<style>` block (Structurizr's order: root, elements,
/// relationships, deployment nodes, boundaries, groups); `legend` gives
/// elements and deployment nodes `MaximumWidth: 200` and adds the
/// transparent class.
fn style_block(w: &mut Writer, c: &Classes, legend: bool) {
    w.line("<style>");
    w.line("  root {");
    w.line("    BackgroundColor: #ffffff;");
    w.line("    FontColor: #444444;");
    w.line("  }");
    for (name, s) in &c.elements {
        let width = if legend { 200 } else { s.width };
        box_lines(w, (name, &element_class(s)), &BoxStyle::from(s), Some(width), true);
    }
    for (name, s) in &c.relationships {
        w.line(&format!("  // {name}"));
        w.line(&format!("  .{} {{", rel_class(s)));
        w.line(&format!("    LineThickness: {};", s.thickness));
        w.line(&format!("    LineStyle: {};", line_style(s.style, s.thickness)));
        w.line(&format!("    LineColor: {};", s.color));
        w.line(&format!("    FontColor: {};", s.color));
        w.line(&format!("    FontSize: {};", s.font_size));
        w.line(&format!("    MaximumWidth: {};", s.width));
        w.line("  }");
    }
    for (name, s) in &c.nodes {
        box_lines(w, (name, &node_class(s)), &BoxStyle::from(s), legend.then_some(200), false);
    }
    for (name, s) in &c.boundaries {
        box_lines(w, (name, &format!("Boundary-{}", base64(name.as_bytes()))), s, None, false);
    }
    for (name, s) in &c.groups {
        box_lines(w, (name, &format!("Group-{}", base64(name.as_bytes()))), s, None, false);
    }
    if legend {
        w.line("  // transparent element for relationships in legend");
        w.line("  .Element-Transparent {");
        w.line("    BackgroundColor: transparent;");
        w.line("    LineColor: transparent;");
        w.line("    FontColor: transparent;");
        w.line("  }");
    }
    w.line("</style>");
}

/// The legend text of a class: its name without the leading base tag,
/// commas spaced.
fn legend_label(name: &str, base: &str) -> String {
    name.strip_prefix(&format!("{base},")).unwrap_or(name).replace(',', ", ")
}

/// The `-key.puml` legend.
fn legend(c: &Classes) -> String {
    let mut w = Writer::default();
    w.line("@startuml");
    w.blank();
    w.line("set separator none");
    w.line("hide stereotype");
    w.blank();
    style_block(&mut w, c, true);
    w.blank();
    for (name, s) in &c.nodes {
        w.line(&format!("rectangle \"=={}\" <<{}>>", legend_label(name, "Element"), node_class(s)));
        w.blank();
    }
    for (name, s) in &c.elements {
        w.line(&format!(
            "{} \"=={}\" <<{}>>",
            shape_keyword(s.shape),
            legend_label(name, "Element"),
            element_class(s)
        ));
        w.blank();
    }
    for (n, (name, s)) in c.relationships.iter().enumerate() {
        let n = n + 1;
        w.line(&format!("rectangle \".\" <<.Element-Transparent>> as {n}"));
        w.line(&format!(
            "{n} --> {n} <<{}>> : \"{}\"",
            rel_class(s),
            legend_label(name, "Relationship")
        ));
        w.blank();
    }
    w.line("@enduml");
    w.finish()
}

/// The `title` and the lines after it that both flavours share.
fn title(w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>) {
    let title = ws.title(view.spec);
    let size = |tag: &str| ws.tag_style(tag).and_then(|s| s.font_size).unwrap_or(24);
    match Workspace::description(view.spec) {
        Some(d) => w.line(&format!(
            "title <size:{}>{title}</size>\\n<size:{}>{d}</size>",
            size("Diagram:Title"),
            size("Diagram:Description")
        )),
        None => w.line(&format!("title <size:{}>{title}</size>", size("Diagram:Title"))),
    }
    w.blank();
    w.line("set separator none");
}

/// The drawn text of a relationship view: `<order>: <description>`.
fn rel_text(r: &ViewRel) -> String {
    match &r.order {
        Some(o) => format!("{o}: {}", r.description),
        None => r.description.clone(),
    }
}

/// The last segment of a group's full name.
fn group_name(ws: &Workspace<'_>, group: &str) -> String {
    match separator(ws) {
        Some(sep) => group.rsplit(sep.as_str()).next().unwrap_or(group).to_owned(),
        None => group.to_owned(),
    }
}

/// The URL of `e` (an instance falls back to its element's), if any.
fn url<'m>(ws: &Workspace<'m>, e: ElementId) -> Option<&'m str> {
    let el = ws.model.element(e);
    el.url
        .as_ref()
        .filter(|u| !u.value.is_empty())
        .or_else(|| el.instance_of.and_then(|of| ws.model.element(of).url.as_ref()))
        .map(|u| u.value.as_str())
        .filter(|u| !u.is_empty())
}

/// The type line in brackets, as `typeOf(view, e, true)`: empty when the
/// type is.
fn bracketed(ws: &Workspace<'_>, e: ElementId) -> String {
    let t = ws.type_line(e);
    if t.is_empty() { t } else { format!("[{t}]") }
}

/// An `<img:...>` for an `http(s)` icon (module comment).
fn img(icon: Option<&str>) -> String {
    icon.filter(|i| i.starts_with("http"))
        .map(|i| format!("\\n\\n<img:{i}{{scale=0.5}}>"))
        .unwrap_or_default()
}

/// `StructurizrPlantUMLExporter`.
struct Structurizr<'c> {
    classes: &'c Classes,
    group: &'c mut u32,
}

impl Diagram for Structurizr<'_> {
    fn header(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>) {
        w.line("@startuml");
        title(w, ws, view);
        w.line("top to bottom direction");
        if view.spec.auto_layout.is_some() {
            w.line("skinparam ranksep 60");
            w.line("skinparam nodesep 30");
        }
        w.line("hide stereotype");
        w.blank();
        style_block(w, self.classes, false);
        w.blank();
    }

    fn element(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let style = ws.style(e);
        let of = ws.model.element(e).instance_of.unwrap_or(e);
        let shape = ws.style(of).shape;
        let mut label = format!("=={}", ws.name(e));
        let kind_line = bracketed(ws, e);
        if !kind_line.is_empty() && style.metadata {
            let _ = write!(label, "\\n<size:{}>{kind_line}</size>", small(style.font_size));
        }
        label.push_str(&img(style.icon.as_deref()));
        let desc = ws.element_description(e);
        if !desc.is_empty() && style.description {
            let _ = write!(label, "\\n\\n{desc}");
        }
        let link = url(ws, e).map(|u| format!(" [[{u}]]")).unwrap_or_default();
        w.line(&format!(
            "{} \"{label}\" <<{}>> as {}{link}",
            shape_keyword(shape),
            element_class(&style),
            ws.alias(e)
        ));
    }

    fn start_group(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, group: &str) {
        *self.group += 1;
        // `findGroupStyle`: the group's own `http(s)` icon, else for an
        // `http(s)` icon on `Group`, `Group`'s colour (Structurizr 6.2.4
        // reads `getColor()` there).
        let http = |s: &TagStyle| s.icon.as_deref().is_some_and(|i| i.starts_with("http"));
        let one = ws.tag_style(&format!("Group:{group}"));
        let all = ws.tag_style("Group");
        let icon = match (one.as_ref().filter(|s| http(s)), all.as_ref().filter(|s| http(s))) {
            (Some(s), _) => s.icon.clone(),
            (None, Some(s)) => s.color.map(|c| c.to_string()),
            _ => None,
        };
        let icon = icon.map(|i| format!("\\n\\n<img:{i}{{scale=0.5}}>")).unwrap_or_default();
        w.line(&format!(
            "rectangle \"{}{icon}\" <<Group-{}>> as group{} {{",
            group_name(ws, group),
            base64(group.as_bytes()),
            self.group
        ));
        w.indent();
    }

    fn end_group(&mut self, w: &mut Writer) {
        w.outdent();
        w.line("}");
        w.blank();
    }

    fn start_boundary(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let name = &ws.model.element(e).name.value;
        let font = boundary_style(ws, e).font_size;
        w.line(&format!(
            "rectangle \"{name}\\n<size:{}>{}</size>\" <<Boundary-{}>> {{",
            small(font),
            bracketed(ws, e),
            base64(name.as_bytes())
        ));
        w.indent();
    }

    fn end_boundary(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn start_node(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let s = ws.style(e);
        let mut name = ws.name(e).to_owned();
        let instances = ws.instances(e).unwrap_or("1");
        if instances != "1" {
            let _ = write!(name, " (x{instances})");
        }
        let link = url(ws, e).map(|u| format!(" [[{u}]]")).unwrap_or_default();
        w.line(&format!(
            "rectangle \"{name}\\n<size:{}>{}</size>{}\" <<{}>> as {}{link} {{",
            small(s.font_size),
            bracketed(ws, e),
            img(s.icon.as_deref()),
            node_class(&s),
            ws.alias(e)
        ));
        w.indent();
    }

    fn end_node(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn relationship(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, r: &ViewRel) {
        let s = ws.rel_style(r.rel);
        let tech = &ws.rels[r.rel].technology;
        let mut label = rel_text(r);
        if !tech.is_empty() {
            let _ = write!(label, "\\n<size:{}>[{tech}]</size>", small(s.font_size));
        }
        let arrow = if r.response { "<--" } else { "-->" };
        w.line(&format!(
            "{} {arrow} {} <<{}>> : \"{label}\"",
            ws.alias(r.source),
            ws.alias(r.destination),
            rel_class(&s)
        ));
    }

    fn footer(&mut self, w: &mut Writer, _: &Workspace<'_>, _: &View<'_>) {
        w.line("@enduml");
    }
}

/// `C4PlantUMLExporter`.
struct C4 {
    group: u32,
}

impl Diagram for C4 {
    fn header(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>) {
        w.line("@startuml");
        title(w, ws, view);
        w.line("top to bottom direction");
        w.blank();
        w.line("<style>");
        w.line("  root {");
        w.line("    BackgroundColor: #ffffff");
        w.line("    FontColor: #444444");
        w.line("  }");
        w.line("</style>");
        w.blank();
        let model = ws.model;
        let has = |f: &dyn Fn(ElementKind) -> bool| view.elements.iter().any(|&e| f(model.element(e).kind));
        w.line("!include <C4/C4>");
        w.line("!include <C4/C4_Context>");
        if has(&|k| matches!(k, ElementKind::Container | ElementKind::ContainerInstance)) {
            w.line("!include <C4/C4_Container>");
        }
        if has(&|k| k == ElementKind::Component) {
            w.line("!include <C4/C4_Component>");
        }
        if view.spec.kind == ViewKind::Deployment {
            w.line("!include <C4/C4_Deployment>");
        }
        w.blank();
    }

    fn element(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let model = ws.model;
        let el = model.element(e);
        let kind = el.instance_of.map_or(el.kind, |of| model.element(of).kind);
        let name = ws.name(e);
        let descr = ws.element_description(e);
        let techn = ws.technology(e);
        let alias = ws.alias(e);
        let link = url(ws, e).unwrap_or("");
        let suffix = match ws.style(e).shape {
            Shape::Cylinder => "Db",
            Shape::Pipe => "Queue",
            _ => "",
        };
        let text = match kind {
            ElementKind::Person => format!("Person({alias}, \"{name}\", $descr=\"{descr}\", $tags=\"\", $link=\"{link}\")"),
            ElementKind::Container | ElementKind::Component => {
                let macro_name = if kind == ElementKind::Container { "Container" } else { "Component" };
                format!(
                    "{macro_name}{suffix}({alias}, \"{name}\", $techn=\"{techn}\", $descr=\"{descr}\", $tags=\"\", $link=\"{link}\")"
                )
            }
            ElementKind::InfrastructureNode => format!(
                "Deployment_Node({alias}, \"{name}\", $type=\"{techn}\", $descr=\"{descr}\", $tags=\"\", $link=\"{link}\")"
            ),
            ElementKind::SoftwareSystem => {
                format!("System({alias}, \"{name}\", $descr=\"{descr}\", $tags=\"\", $link=\"{link}\")")
            }
            _ => return,
        };
        w.line(&text);
    }

    fn start_group(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, group: &str) {
        self.group += 1;
        let one = ws.tag_style(&format!("Group:{group}"));
        let all = ws.tag_style("Group");
        let (one, all) = (one.as_ref(), all.as_ref());
        let color = first(one, all, |s| s.stroke)
            .or_else(|| first(one, all, |s| s.color))
            .map_or_else(|| "#cccccc".to_owned(), |c| c.to_string());
        let thickness = first(one, all, |s| s.stroke_width).unwrap_or(1);
        let mut line = format!(
            "AddBoundaryTag(\"{group}\", $borderColor=\"{color}\", $fontColor=\"{color}\", $borderStyle=\"dashed\""
        );
        if thickness != 1 {
            let _ = write!(line, ", $borderThickness=\"{thickness}\"");
        }
        line.push(')');
        w.line(&line);
        w.line(&format!(
            "Boundary(group_{}, \"{}\", $tags=\"{group}\") {{",
            self.group,
            group_name(ws, group)
        ));
        w.indent();
    }

    fn end_group(&mut self, w: &mut Writer) {
        w.outdent();
        w.line("}");
        w.blank();
    }

    fn start_boundary(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let el = ws.model.element(e);
        let macro_name = if el.kind == ElementKind::Container { "Container_Boundary" } else { "System_Boundary" };
        w.line(&format!(
            "{macro_name}(\"{}_boundary\", \"{}\", $tags=\"\") {{",
            ws.alias(e),
            el.name.value
        ));
        w.indent();
    }

    fn end_boundary(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn start_node(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let mut name = ws.name(e).to_owned();
        let instances = ws.instances(e).unwrap_or("1");
        if instances != "1" {
            let _ = write!(name, " (x{instances})");
        }
        w.line(&format!(
            "Deployment_Node({}, \"{name}\", $type=\"{}\", $descr=\"{}\", $tags=\"\", $link=\"{}\") {{",
            ws.alias(e),
            ws.technology(e),
            ws.element_description(e),
            url(ws, e).unwrap_or("")
        ));
        w.indent();
    }

    fn end_node(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn relationship(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, r: &ViewRel) {
        let model = ws.model;
        if model.element(r.source).kind == ElementKind::Custom || model.element(r.destination).kind == ElementKind::Custom {
            return;
        }
        let (s, d) = if r.response { (r.destination, r.source) } else { (r.source, r.destination) };
        let link = ws.rels[r.rel]
            .own
            .and_then(|o| model.relationship(o).url.as_ref())
            .map_or("", |u| u.value.as_str());
        w.line(&format!(
            "Rel({}, {}, \"{}\", $techn=\"{}\", $tags=\"\", $link=\"{link}\")",
            ws.alias(s),
            ws.alias(d),
            rel_text(r),
            ws.rels[r.rel].technology
        ));
    }

    fn footer(&mut self, w: &mut Writer, _: &Workspace<'_>, _: &View<'_>) {
        w.line("SHOW_LEGEND(true)");
        w.line("hide stereotypes");
        w.line("@enduml");
    }
}
