//! Mermaid, as Structurizr 6.2.4's `MermaidDiagramExporter` writes it
//! (P3-408), read from `structurizr-export` 6.2.4's sources.

// DECISION: texts are written raw, as Structurizr writes them (ruling
// E1); `mermaid.title`, `mermaid.icons` and `mermaid.sequenceDiagram`
// keep their defaults (not read).

use std::fmt::Write as _;

use crate::model::{Border, ElementId, ElementKind, Shape};

use super::super::structurizr::{View, ViewRel, Workspace, Writer};
use super::{Diagram, separator, walk};

pub(super) fn write(ws: &Workspace<'_>, view: &View<'_>) -> String {
    walk(ws, view, &mut Mermaid { group: 0 }).finish()
}

/// `lines`: words (split on white space) on lines shorter than 30
/// characters, joined with `<br />`.
fn lines(text: &str) -> String {
    let mut out = String::new();
    let mut line = String::new();
    let len = |s: &str| s.encode_utf16().count();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if len(&line) + len(word) + 1 < 30 {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push_str(&line);
            out.push_str("<br />");
            line.clear();
            line.push_str(word);
        }
    }
    out.push_str(&line);
    out
}

struct Mermaid {
    group: u32,
}

impl Diagram for Mermaid {
    fn header(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>) {
        w.line("graph TB");
        w.indent();
        w.line("linkStyle default fill:#ffffff");
        w.blank();
        w.line(&format!("subgraph diagram [\"{}\"]", ws.title(view.spec)));
        w.indent();
        w.line("style diagram fill:#ffffff,stroke:#ffffff");
        w.blank();
    }

    fn element(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let s = ws.style(e);
        let id = ws.id(e);
        let (open, close) = match s.shape {
            Shape::RoundedBox => ("(", ")"),
            Shape::Cylinder => ("[(", ")]"),
            _ => ("[", "]"),
        };
        let mut content = format!("<div style='font-weight: bold'>{}</div>", ws.name(e));
        if s.metadata {
            let t = ws.type_line(e);
            let t = if t.is_empty() { t } else { format!("[{t}]") };
            let _ = write!(content, "<div style='font-size: 70%; margin-top: 0px'>{t}</div>");
        }
        let d = ws.element_description(e);
        if !d.is_empty() && s.description {
            let _ = write!(content, "<div style='font-size: 80%; margin-top:10px'>{}</div>", lines(d));
        }
        w.line(&format!("{id}{open}\"{content}\"{close}"));
        if let Some(url) = ws.model.element(e).url.as_ref().filter(|u| !u.value.is_empty()) {
            w.line(&format!("click {id} {} \"{}\"", url.value, url.value));
        }
        w.line(&format!("style {id} fill:{},stroke:{},color:{}", s.background, s.stroke, s.color));
    }

    fn start_group(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, group: &str) {
        self.group += 1;
        let name = match separator(ws) {
            Some(sep) => group.rsplit(sep.as_str()).next().unwrap_or(group).to_owned(),
            None => group.to_owned(),
        };
        let color = ws
            .tag_style(&format!("Group:{group}"))
            .and_then(|s| s.color)
            .or_else(|| ws.tag_style("Group").and_then(|s| s.color))
            .map_or_else(|| "#cccccc".to_owned(), |c| c.to_string());
        let n = self.group;
        w.line(&format!("subgraph group{n} [\"{name}\"]"));
        w.indent();
        w.line(&format!("style group{n} fill:#ffffff,stroke:{color},color:{color},stroke-dasharray:5"));
        w.blank();
    }

    fn end_group(&mut self, w: &mut Writer) {
        w.outdent();
        w.line("end");
        w.blank();
    }

    fn start_boundary(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let s = ws.style(e);
        let id = ws.id(e);
        w.line(&format!("subgraph {id} [\"{}\"]", ws.model.element(e).name.value));
        w.indent();
        w.line(&format!("style {id} fill:#ffffff,stroke:{},color:{}", s.stroke, s.stroke));
        w.blank();
    }

    fn end_boundary(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn start_node(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, e: ElementId) {
        let s = ws.style(e);
        let id = ws.id(e);
        w.line(&format!("subgraph {id} [\"{}\"]", ws.model.element(e).name.value));
        w.indent();
        w.line(&format!("style {id} fill:#ffffff,stroke:{},color:{}", s.stroke, s.color));
        w.blank();
    }

    fn end_node(&mut self, w: &mut Writer) {
        self.end_group(w);
    }

    fn relationship(&mut self, w: &mut Writer, ws: &Workspace<'_>, _: &View<'_>, r: &ViewRel) {
        let model = ws.model;
        let node = |x: ElementId| model.element(x).kind == ElementKind::DeploymentNode;
        if node(r.source) || node(r.destination) {
            return;
        }
        let (from, to) = if r.response { (r.destination, r.source) } else { (r.source, r.destination) };
        let solid = ws.rel_style(r.rel).style == Border::Solid;
        let description = match &r.order {
            Some(o) => format!("{o}. {}", r.description),
            None => r.description.clone(),
        };
        let tech = &ws.rels[r.rel].technology;
        let tech = if tech.is_empty() { String::new() } else { format!("[{tech}]") };
        let (open, close) = if solid { ("-", "-") } else { (".", ".") };
        w.line(&format!(
            "{}-{open} \"<div>{}</div><div style='font-size: 70%'>{tech}</div>\" {close}->{}",
            ws.id(from),
            lines(&description),
            ws.id(to)
        ));
    }

    fn footer(&mut self, w: &mut Writer, _: &Workspace<'_>, _: &View<'_>) {
        w.outdent();
        w.line("end");
        w.outdent();
    }
}
