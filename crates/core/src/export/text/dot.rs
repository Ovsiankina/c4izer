//! Graphviz DOT, as `structurizr-export` 6.2.4's `DOTExporter` writes it
//! (P3-410), read from its sources, with c4rizer's `--positions`
//! (P3-411).

// DECISION: only an element's name and description are escaped (`"` as
// `\"`), as `DOTExporter.escape` does; other texts go in raw (ruling E1).
// DECISION: `pos` (P3-411) is the box centre in px with y negated, as
// Graphviz's y axis points up, so the drawing is not mirrored.

use std::fmt::Write as _;

use crate::model::{Border, ElementId, ElementKind, Shape, ViewKind, ViewScope};
use crate::scene::Scene;

use super::super::structurizr::{View, ViewRel, Workspace, Writer, break_text};
use super::{Diagram, separator, walk};

pub(super) fn write(ws: &Workspace<'_>, view: &View<'_>, scene: Option<&Scene>) -> String {
    walk(ws, view, &mut Dot { scene }).finish()
}

/// `DOTExporter.escape`.
fn escape(s: &str) -> String {
    s.replace('"', "\\\"")
}

struct Dot<'s> {
    scene: Option<&'s Scene>,
}

impl Dot<'_> {
    fn cluster(w: &mut Writer, head: &str, label: &str, colours: [&str; 3], dashed: bool) {
        w.line(head);
        w.indent();
        w.line("margin=25");
        w.line(&format!("label=<{label}>"));
        w.line("labelloc=b");
        let [color, font, fill] = colours;
        w.line(&format!("color=\"{color}\""));
        w.line(&format!("fontcolor=\"{font}\""));
        w.line(&format!("fillcolor=\"{fill}\""));
        if dashed {
            w.line("style=\"dashed\"");
        }
        w.blank();
    }

    /// `, pos="x,y!"` for `e`'s box, when positions are asked for.
    fn pos(&self, e: ElementId) -> String {
        let Some(scene) = self.scene else { return String::new() };
        let Some(b) = scene.boxes.iter().find(|b| b.element == e) else {
            return String::new();
        };
        let x = i32::midpoint(b.rect.min.x, b.rect.max.x);
        let y = i32::midpoint(b.rect.min.y, b.rect.max.y);
        format!(", pos=\"{x},{}!\"", -y)
    }
}

/// `findElementInside`: an instance or infrastructure node on the view
/// inside deployment node `node`, depth first.
fn inside(ws: &Workspace<'_>, view: &View<'_>, node: ElementId) -> Option<ElementId> {
    let model = ws.model;
    let children = |k: ElementKind| -> Vec<ElementId> {
        model.elements.iter().filter(|c| c.parent == Some(node) && c.kind == k).map(|c| c.id).collect()
    };
    for k in [ElementKind::SoftwareSystemInstance, ElementKind::ContainerInstance, ElementKind::InfrastructureNode] {
        if let Some(&e) = children(k).iter().find(|&&e| view.elements.contains(&e)) {
            return Some(e);
        }
    }
    children(ElementKind::DeploymentNode).into_iter().find_map(|c| inside(ws, view, c))
}

impl Diagram for Dot<'_> {
    fn header(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>) {
        let description = Workspace::description(view.spec)
            .map(|d| format!("<br /><font point-size=\"24\">{d}</font>"))
            .unwrap_or_default();
        w.line("digraph {");
        w.indent();
        w.line("compound=true");
        w.line("graph [fontname=\"Arial\", rankdir=TB, ranksep=1.0, nodesep=1.0]");
        w.line("node [fontname=\"Arial\", shape=box, margin=\"0.4,0.3\"]");
        w.line("edge [fontname=\"Arial\"]");
        w.line(&format!(
            "label=<<br /><font point-size=\"34\">{}</font>{description}>",
            ws.title(view.spec)
        ));
        w.blank();
    }

    fn element(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let style = ws.style(e);
        let id = ws.id(e);
        let of = ws.model.element(e).instance_of.unwrap_or(e);
        let shape = match ws.style(of).shape {
            Shape::Circle => "circle",
            Shape::Component => "component",
            Shape::Cylinder => "cylinder",
            Shape::Ellipse => "ellipse",
            Shape::Folder => "folder",
            Shape::Hexagon => "hexagon",
            Shape::Diamond => "diamond",
            _ => "rect",
        };
        let font = style.font_size;
        let width = style.width;
        let name = ws.name(e);
        let mut label = if name.is_empty() {
            String::new()
        } else {
            format!("<font point-size=\"{}\">{}</font>", font + 10, break_text(width, font + 10, &escape(name)))
        };
        let t = ws.type_line(e);
        if !t.is_empty() && style.metadata {
            let _ = write!(label, "<br /><font point-size=\"{}\">[{t}]</font>", font.saturating_sub(5));
        }
        let desc = ws.element_description(e);
        if !desc.is_empty() && style.description {
            let _ = write!(label, "<br /><br /><font point-size=\"{font}\">{}</font>", break_text(width, font, &escape(desc)));
        }
        w.line(&format!(
            "{id} [id={id},shape={shape}, label=<{label}>, style=filled, color=\"{}\", fillcolor=\"{}\", fontcolor=\"{}\"{}]",
            style.stroke,
            style.background,
            style.color,
            self.pos(e)
        ));
    }

    fn start_group(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, group: &str) {
        let name = match separator(ws) {
            Some(sep) => group.rsplit(sep.as_str()).next().unwrap_or(group).to_owned(),
            None => group.to_owned(),
        };
        let color = ws
            .tag_style(&format!("Group:{group}"))
            .and_then(|s| s.color)
            .or_else(|| ws.tag_style("Group").and_then(|s| s.color))
            .map_or_else(|| "#cccccc".to_owned(), |c| c.to_string());
        Dot::cluster(
            w,
            &format!("subgraph \"cluster_group_{name}\" {{"),
            &format!("<font point-size=\"24\"><br />{name}</font>"),
            [&color, &color, "#ffffff"],
            true,
        );
    }

    fn end_group(&mut self, w: &mut Writer) {
        w.outdent();
        w.line("}");
        w.blank();
    }

    fn start_boundary(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, e: ElementId) {
        let model = ws.model;
        let scope = view.spec.scope.element();
        let scope_system = match scope {
            Some(s) if model.element(s).kind == ElementKind::SoftwareSystem => Some(s),
            Some(c) => model.element(c).parent,
            None => None,
        };
        let main = if model.element(e).kind == ElementKind::SoftwareSystem {
            scope_system == Some(e)
        } else {
            match (view.spec.kind, &view.spec.scope) {
                (ViewKind::Component | ViewKind::Dynamic, ViewScope::Element(s)) => s.value == e,
                _ => true,
            }
        };
        let color = if main { "#444444" } else { "#cccccc" };
        let label = format!(
            "<font point-size=\"24\"><br />{}</font><br /><font point-size=\"19\">[{}]</font>",
            model.element(e).name.value,
            ws.type_line(e)
        );
        Dot::cluster(w, &format!("subgraph cluster_{} {{", ws.id(e)), &label, [color, color, color], false);
    }

    fn end_boundary(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn start_node(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let s = ws.style(e);
        let label = format!(
            "<font point-size=\"24\">{}</font><br /><font point-size=\"19\">[{}]</font>",
            ws.name(e),
            ws.type_line(e)
        );
        let (stroke, color) = (s.stroke.to_string(), s.color.to_string());
        Dot::cluster(w, &format!("subgraph cluster_{} {{", ws.id(e)), &label, [&stroke, &color, "#ffffff"], false);
    }

    fn end_node(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn relationship(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, r: &ViewRel) {
        let model = ws.model;
        let s = ws.rel_style(r.rel);
        let rel = &ws.rels[r.rel];
        let description = match &r.order {
            Some(o) => format!("{o}. {}", r.description),
            None => r.description.clone(),
        };
        let font = s.font_size;
        let mut label = if description.is_empty() {
            String::new()
        } else {
            format!("<font point-size=\"{font}\">{}</font>", break_text(400, font, &description))
        };
        if !rel.technology.is_empty() {
            let _ = write!(label, "<br /><font point-size=\"{}\">[{}]</font>", font.saturating_sub(5), rel.technology);
        }
        let node = |e: ElementId| model.element(e).kind == ElementKind::DeploymentNode;
        let mut cluster = String::new();
        let (source, destination) = if node(r.source) || node(r.destination) {
            let source = if node(r.source) { inside(ws, view, r.source) } else { Some(r.source) };
            let destination = if node(r.destination) { inside(ws, view, r.destination) } else { Some(r.destination) };
            if source.is_some() && destination.is_some() {
                if node(r.source) {
                    let _ = write!(cluster, ",ltail=cluster_{}", ws.id(r.source));
                }
                if node(r.destination) {
                    let _ = write!(cluster, ",lhead=cluster_{}", ws.id(r.destination));
                }
            }
            (source, destination)
        } else if r.response {
            (Some(r.destination), Some(r.source))
        } else {
            (Some(r.source), Some(r.destination))
        };
        // Structurizr fails on a deployment node with nothing inside; such
        // a line is left out.
        let (Some(source), Some(destination)) = (source, destination) else { return };
        let style = if s.style == Border::Solid { "solid" } else { "dashed" };
        w.line(&format!(
            "{} -> {} [id={}, label=<{label}>, style=\"{style}\", color=\"{}\", fontcolor=\"{}\"{cluster}]",
            ws.id(source),
            ws.id(destination),
            rel.id,
            s.color,
            s.color
        ));
    }

    fn footer(&mut self, w: &mut Writer, _: &Workspace<'_>, _: &View<'_>) {
        w.outdent();
        w.line("}");
    }
}
