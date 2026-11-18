//! Ilograph, as `structurizr-export` 6.2.4's `IlographExporter` writes it
//! (P3-412), read from its sources: resources nested as the model, one
//! `Static Structure` perspective, one sequence perspective per dynamic
//! view, one perspective per deployment environment.

// DECISION: Structurizr's indentation is kept as it falls out of its
// writer, including a deployment node that holds nothing outdenting
// twice (`structurizr::Writer` writes a level below zero at column
// zero).
// DECISION: `ilograph.imports` (a workspace property) is not read.

use crate::model::{ElementId, ElementKind, ViewKind};

use super::super::structurizr::{Rel, Workspace, Writer};

#[allow(clippy::too_many_lines)] // one pass of Structurizr's `export`, kept whole
pub(super) fn write(ws: &Workspace<'_>) -> String {
    let model = ws.model;
    let mut w = Writer::default();
    w.line("resources:");
    w.indent();
    let top = |k: ElementKind| -> Vec<ElementId> {
        let mut out: Vec<ElementId> =
            model.elements.iter().filter(|e| e.parent.is_none() && e.kind == k).map(|e| e.id).collect();
        ws.sort(&mut out);
        out
    };
    // The elements whose relationships the static perspective reads.
    let mut elements: Vec<ElementId> = Vec::new();
    for k in [ElementKind::Custom, ElementKind::Person] {
        for e in top(k) {
            fields(&mut w, ws, e);
            elements.push(e);
        }
    }
    for s in top(ElementKind::SoftwareSystem) {
        fields(&mut w, ws, s);
        elements.push(s);
        let containers = children(ws, s, |k| k == ElementKind::Container);
        if !containers.is_empty() {
            w.indent();
            w.line("children:");
            w.indent();
            for c in containers {
                fields(&mut w, ws, c);
                elements.push(c);
                let components = children(ws, c, |k| k == ElementKind::Component);
                if !components.is_empty() {
                    w.indent();
                    w.line("children:");
                    w.indent();
                    for x in components {
                        fields(&mut w, ws, x);
                        elements.push(x);
                    }
                    w.outdent();
                    w.outdent();
                }
            }
            w.outdent();
            w.outdent();
        }
    }
    let nodes = top(ElementKind::DeploymentNode);
    for &n in &nodes {
        node(&mut w, ws, n);
    }
    // Relationships between people, systems and custom elements, then
    // with containers, then with components, each once.
    let level = |e: ElementId| match model.element(e).kind {
        ElementKind::Container => 2,
        ElementKind::Component => 3,
        ElementKind::Person | ElementKind::SoftwareSystem | ElementKind::Custom => 1,
        _ => 9,
    };
    let mut relations: Vec<usize> = Vec::new();
    for pass in 1..=3 {
        for &e in &elements {
            for i in efferent(ws, e) {
                let r = &ws.rels[i];
                if level(r.source) <= pass && level(r.destination) <= pass && !relations.contains(&i) {
                    relations.push(i);
                }
            }
        }
    }
    w.outdent();
    w.line("perspectives:");
    w.indent();
    w.line("- name: Static Structure");
    w.indent();
    w.line("relations:");
    w.indent();
    for i in relations {
        relation(&mut w, ws, i);
        w.blank();
        w.outdent();
    }
    w.outdent();
    w.outdent();
    w.outdent();
    for spec in model.views.iter().filter(|v| v.kind == ViewKind::Dynamic) {
        let Some(view) = ws.view(spec) else { continue };
        let name = ws.view_name(spec);
        let rest = name.strip_prefix("Dynamic View").unwrap_or(&name);
        let scope = rest.strip_prefix(": ").unwrap_or(rest);
        w.indent();
        w.line(&format!("- name: Dynamic: {scope}"));
        w.indent();
        w.line("sequence:");
        for (n, r) in view.relationships.iter().enumerate() {
            let rel = &ws.rels[r.rel];
            if n == 0 {
                w.indent();
                w.line(&format!("start: \"{}\"", ws.id(rel.source)));
                w.line("steps:");
                w.line(&format!("- to: \"{}\"", ws.id(rel.destination)));
            } else if r.response {
                w.line(&format!("- to: \"{}\"", ws.id(rel.source)));
            } else {
                w.line(&format!("- to: \"{}\"", ws.id(rel.destination)));
            }
            w.indent();
            if !r.description.is_empty() {
                w.line(&format!("label: \"{}. {}\"", r.order.as_deref().unwrap_or(""), r.description));
            }
            if !rel.technology.is_empty() {
                w.line(&format!("description: \"{}\"", rel.technology));
            }
            w.line(&format!("color: \"{}\"", ws.rel_style(r.rel).color));
            w.outdent();
            w.blank();
        }
        w.outdent();
        w.outdent();
        w.outdent();
    }
    let mut environments: Vec<&str> = model
        .elements
        .iter()
        .filter(|e| e.parent.is_none() && e.kind == ElementKind::DeploymentNode)
        .filter_map(|e| e.environment.map(|i| model.environments[i.0 as usize].name.value.as_str()))
        .collect();
    environments.sort_unstable();
    environments.dedup();
    for env in environments {
        w.indent();
        w.line(&format!("- name: Deployment - {env}"));
        w.indent();
        w.line("relations:");
        let mut list: Vec<ElementId> = nodes
            .iter()
            .copied()
            .filter(|&n| model.element(n).environment.is_some_and(|i| model.environments[i.0 as usize].name.value == env))
            .collect();
        for n in list.clone() {
            all_children(ws, n, &mut list);
        }
        w.indent();
        for &e in &list {
            for i in efferent(ws, e) {
                if list.contains(&ws.rels[i].destination) {
                    w.line(&format!("- from: \"{}\"", ws.id(ws.rels[i].source)));
                    w.indent();
                    relation_fields(&mut w, ws, &ws.rels[i], i);
                    w.outdent();
                }
            }
        }
        w.outdent();
        w.outdent();
        w.outdent();
    }
    w.finish()
}

/// The relationships from `e`, sorted by id as strings.
fn efferent(ws: &Workspace<'_>, e: ElementId) -> Vec<usize> {
    let mut out: Vec<usize> = (0..ws.rels.len()).filter(|&i| ws.rels[i].source == e).collect();
    out.sort_by_cached_key(|&i| ws.rels[i].id.to_string());
    out
}

fn relation(w: &mut Writer, ws: &Workspace<'_>, i: usize) {
    let r = &ws.rels[i];
    w.line(&format!("- from: \"{}\"", ws.id(r.source)));
    w.indent();
    relation_fields(w, ws, r, i);
}

fn relation_fields(w: &mut Writer, ws: &Workspace<'_>, r: &Rel, i: usize) {
    w.line(&format!("to: \"{}\"", ws.id(r.destination)));
    if !r.description.is_empty() {
        w.line(&format!("label: \"{}\"", r.description));
    }
    if !r.technology.is_empty() {
        w.line(&format!("description: \"{}\"", r.technology));
    }
    w.line(&format!("color: \"{}\"", ws.rel_style(i).color));
}

/// `writeElement`: one resource's own lines and its blank line.
fn fields(w: &mut Writer, ws: &Workspace<'_>, e: ElementId) {
    let style = ws.style(e);
    w.line(&format!("- id: \"{}\"", ws.id(e)));
    w.indent();
    w.line(&format!("name: \"{}\"", ws.name(e)));
    let t = ws.type_line(e);
    let t = if t.is_empty() { t } else { format!("[{t}]") };
    w.line(&format!("subtitle: \"{t}\""));
    let d = ws.element_description(e);
    if !d.is_empty() {
        w.line(&format!("description: \"{d}\""));
    }
    if ws.model.element(e).kind == ElementKind::DeploymentNode {
        w.line("backgroundColor: \"#ffffff\"");
    } else {
        w.line(&format!("backgroundColor: \"{}\"", style.background));
    }
    w.line(&format!("color: \"{}\"", style.color));
    if let Some(icon) = style.ilograph_icon.clone().filter(|i| !i.is_empty()).or(style.icon) {
        w.line(&format!("icon: \"{icon}\""));
    }
    w.blank();
    w.outdent();
}

/// The children of `e` that `f` accepts, sorted by id as strings.
fn children(ws: &Workspace<'_>, e: ElementId, f: impl Fn(ElementKind) -> bool) -> Vec<ElementId> {
    let mut out: Vec<ElementId> = ws
        .model
        .elements
        .iter()
        .filter(|c| c.parent == Some(e) && f(c.kind))
        .map(|c| c.id)
        .collect();
    ws.sort(&mut out);
    out
}

/// `writeDeploymentNode`.
fn node(w: &mut Writer, ws: &Workspace<'_>, e: ElementId) {
    fields(w, ws, e);
    if ws.model.elements.iter().any(|c| c.parent == Some(e)) {
        w.indent();
        w.line("children:");
        w.indent();
    }
    for c in children(ws, e, |k| k == ElementKind::DeploymentNode) {
        node(w, ws, c);
    }
    for k in [ElementKind::InfrastructureNode, ElementKind::SoftwareSystemInstance, ElementKind::ContainerInstance] {
        for c in children(ws, e, |x| x == k) {
            fields(w, ws, c);
        }
    }
    w.outdent();
    w.outdent();
}

/// `findAllChildren`: below `e`, each child node's own, then the
/// instances and infrastructure nodes of `e`.
fn all_children(ws: &Workspace<'_>, e: ElementId, out: &mut Vec<ElementId>) {
    for c in children(ws, e, |k| k == ElementKind::DeploymentNode) {
        all_children(ws, c, out);
    }
    for k in [ElementKind::SoftwareSystemInstance, ElementKind::ContainerInstance, ElementKind::InfrastructureNode] {
        out.extend(children(ws, e, |x| x == k));
    }
}
