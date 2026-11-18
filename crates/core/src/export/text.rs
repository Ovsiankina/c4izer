//! Structurizr's text export formats (SRS 4.7, row exporters):
//! `PlantUML` in both flavours, Mermaid, `WebSequenceDiagrams`, DOT and
//! Ilograph,
//! each byte-equal to Structurizr 6.2.4 on the fixtures (ruling E1,
//! P3-406..P3-412).
//!
//! Depends on: `model`, `pipeline`.

// Shape of the API: one function per format, `(&Model, &AllViews, ..)`
// to the files Structurizr's CLI would write, each `(file name, text)`
// in view declaration order. The CLI writes them into the output folder
// (P3-400) and prints its own skip lines (P3-405); views a format cannot
// express give no file. Every text is deterministic with `\n` line
// endings and a final newline (P3-403). No text format carries c4rizer
// coordinates, except `dot` with `positions` (P3-411).
//
// DECISION: the walk over a view (which elements are written where,
// which boundaries open, in which order, where the blank lines go) is
// Structurizr's `AbstractDiagramExporter`, shared here by PlantUML,
// Mermaid and DOT; each format only says how one element, boundary or
// line is written (`Diagram`).
// DECISION: every text ends with one newline (P3-403), where Structurizr
// writes none after `@enduml`, `}` or `end`: the one accepted byte
// difference besides the SRS's (tests compare against the reference
// with its missing final newline added).
// DECISION: Structurizr 6.2.4's own oddities are kept byte for byte, the
// duplicated elements of a container-scoped dynamic view (`comp`) and the
// Ilograph indentation after a deployment node with infrastructure nodes
// included: P3-406 accepts a difference there but does not ask for one.

mod dot;
mod ilograph;
mod mermaid;
mod plantuml;
mod wsd;

use crate::model::{ElementId, ElementKind, Model, ViewKind};
use crate::pipeline::AllViews;

use super::Flavour;
use super::structurizr::{View, ViewRel, Workspace, Writer};

/// The `PlantUML` files of every drawable view: `structurizr-<key>.puml`,
/// and with `Flavour::Structurizr` also `structurizr-<key>-key.puml`
/// (P3-404). Image and filtered views give no file (P3-405). Serves
/// P3-404, P3-406, P3-407.
pub fn plantuml(model: &Model, all: &AllViews, flavour: Flavour) -> Vec<(String, String)> {
    let _ = all;
    let ws = Workspace::new(model);
    let mut out: Vec<(usize, String, String)> = Vec::new();
    // Structurizr numbers groups across the views of one export.
    let mut groups = 0;
    for (i, view) in structurizr_order(&ws) {
        let key = &view.spec.key;
        match flavour {
            Flavour::Structurizr => {
                let (diagram, legend) = plantuml::structurizr(&ws, &view, &mut groups);
                out.push((i, format!("structurizr-{key}.puml"), diagram));
                out.push((i, format!("structurizr-{key}-key.puml"), legend));
            }
            Flavour::C4PlantUml => {
                // `C4PlantUMLExporter` writes no custom view.
                if view.spec.kind != ViewKind::Custom {
                    out.push((i, format!("structurizr-{key}.puml"), plantuml::c4(&ws, &view)));
                }
            }
        }
    }
    out.sort_by_key(|(i, _, _)| *i);
    out.into_iter().map(|(_, n, t)| (n, t)).collect()
}

/// The Mermaid file `structurizr-<key>.mmd` of every drawable view, node
/// ids being Structurizr's numeric element ids (P3-408). Serves P3-404,
/// P3-408.
pub fn mermaid(model: &Model, all: &AllViews) -> Vec<(String, String)> {
    let _ = all;
    let ws = Workspace::new(model);
    model
        .views
        .iter()
        .filter_map(|spec| {
            let view = ws.view(spec)?;
            Some((format!("structurizr-{}.mmd", spec.key), mermaid::write(&ws, &view)))
        })
        .collect()
}

/// The `WebSequenceDiagrams` file `structurizr-<key>.wsd` of every dynamic
/// view (P3-409); other views give no file (P3-405). Serves P3-404,
/// P3-409.
pub fn websequencediagrams(model: &Model, all: &AllViews) -> Vec<(String, String)> {
    let _ = all;
    let ws = Workspace::new(model);
    model
        .views
        .iter()
        .filter(|spec| spec.kind == ViewKind::Dynamic)
        .filter_map(|spec| {
            let view = ws.view(spec)?;
            Some((format!("structurizr-{}.wsd", spec.key), wsd::write(&ws, &view)))
        })
        .collect()
}

/// The Graphviz file `structurizr-<key>.dot` of every drawable view, as
/// `structurizr-export` 6.2.4 `DOTExporter` writes it (P3-410); with
/// `positions`, each node also gets `pos="<x>,<y>!"` from its box in the
/// view's scene (P3-411). Serves P3-404, P3-410, P3-411.
pub fn dot(model: &Model, all: &AllViews, positions: bool) -> Vec<(String, String)> {
    let ws = Workspace::new(model);
    model
        .views
        .iter()
        .filter_map(|spec| {
            let view = ws.view(spec)?;
            let scene = if positions { all.view(&spec.key).map(|b| &b.scene) } else { None };
            Some((format!("structurizr-{}.dot", spec.key), dot::write(&ws, &view, scene)))
        })
        .collect()
}

/// The one Ilograph file `workspace.idl.yaml` of the whole workspace, as
/// `structurizr-export` 6.2.4 `IlographExporter` writes it (P3-412).
/// Serves P3-412.
pub fn ilograph(model: &Model, all: &AllViews) -> Vec<(String, String)> {
    let _ = all;
    let ws = Workspace::new(model);
    vec![("workspace.idl.yaml".to_owned(), ilograph::write(&ws))]
}

/// How one format writes the parts of a diagram; `walk` decides the
/// order (Structurizr's `AbstractDiagramExporter`).
trait Diagram {
    fn header(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>);
    fn element(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, e: ElementId);
    /// A group (`group` is its full name).
    fn start_group(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, group: &str);
    fn end_group(&mut self, w: &mut Writer);
    /// A software system or container drawn as a boundary.
    fn start_boundary(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, e: ElementId);
    fn end_boundary(&mut self, w: &mut Writer);
    /// A deployment node.
    fn start_node(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, e: ElementId);
    fn end_node(&mut self, w: &mut Writer);
    fn relationship(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, r: &ViewRel);
    fn footer(&mut self, w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>);
}

/// The group separator (`structurizr.groupSeparator`), if any.
fn separator(ws: &Workspace<'_>) -> Option<String> {
    ws.model
        .model_properties
        .iter()
        .rev()
        .find(|p| p.name.value == "structurizr.groupSeparator")
        .map(|p| p.value.value.clone())
        .filter(|s| !s.is_empty())
}

/// The full name of `e`'s group, if any.
fn group_of<'m>(ws: &Workspace<'m>, e: ElementId) -> Option<&'m str> {
    ws.model
        .element(e)
        .group
        .map(|g| ws.model.groups[g.0 as usize].full_name.as_str())
        .filter(|g| !g.is_empty())
}

/// One element, or a deployment node with what it holds
/// (`AbstractDiagramExporter.write`).
fn write_one(w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, d: &mut impl Diagram, e: ElementId) {
    if view.spec.kind == ViewKind::Deployment && ws.model.element(e).kind == ElementKind::DeploymentNode {
        node(w, ws, view, d, e);
    } else {
        d.element(w, ws, view, e);
    }
}

/// Structurizr's `writeElements`: sorted by id, the groups first (sorted
/// by name, nested by the separator), then the elements in no group.
fn write_elements(w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, d: &mut impl Diagram, elements: &[ElementId]) {
    let mut elements = elements.to_vec();
    ws.sort(&mut elements);
    let sep = separator(ws);
    let mut groups: Vec<String> = Vec::new();
    for &e in &elements {
        if let Some(g) = group_of(ws, e) {
            let mut g = g.to_owned();
            if !groups.contains(&g) {
                groups.push(g.clone());
            }
            if let Some(sep) = &sep {
                while let Some(at) = g.rfind(sep.as_str()) {
                    g.truncate(at);
                    if !groups.contains(&g) {
                        groups.push(g.clone());
                    }
                }
            }
        }
    }
    groups.sort();
    let members = |w: &mut Writer, d: &mut dyn FnMut(&mut Writer, ElementId), group: &str| {
        for &e in &elements {
            if group_of(ws, e) == Some(group) {
                d(w, e);
            }
        }
    };
    if !groups.is_empty() {
        if let Some(sep) = &sep {
            // Java's `split` drops trailing empty strings; an empty
            // context counts as one part.
            let parts = |g: &str| g.split(sep.as_str()).count().max(1);
            let mut context = String::new();
            for group in &groups {
                let (gc, cc) = (parts(group), parts(&context));
                match gc.cmp(&cc) {
                    std::cmp::Ordering::Greater => w.indent(),
                    std::cmp::Ordering::Equal => {
                        if context.len() > 1 {
                            d.end_group(w);
                        }
                    }
                    std::cmp::Ordering::Less => {
                        for _ in 0..cc - gc {
                            d.end_group(w);
                            w.outdent();
                        }
                        d.end_group(w);
                    }
                }
                d.start_group(w, ws, view, group);
                members(w, &mut |w, e| write_one(w, ws, view, d, e), group);
                context.clone_from(group);
            }
            let cc = parts(&context);
            for i in 0..cc {
                d.end_group(w);
                if i < cc - 1 {
                    w.outdent();
                }
            }
        } else {
            for group in &groups {
                d.start_group(w, ws, view, group);
                members(w, &mut |w, e| write_one(w, ws, view, d, e), group);
                d.end_group(w);
            }
        }
    }
    for &e in &elements {
        if group_of(ws, e).is_none() {
            write_one(w, ws, view, d, e);
        }
    }
}

/// Writes `view` through `d`, in Structurizr's order.
fn walk(ws: &Workspace<'_>, view: &View<'_>, d: &mut impl Diagram) -> Writer {
    let mut w = Writer::default();
    d.header(&mut w, ws, view);
    let model = ws.model;
    let kind = |e: ElementId| model.element(e).kind;
    let parent = |e: ElementId| model.element(e).parent;
    let pick = |f: &dyn Fn(ElementId) -> bool| -> Vec<ElementId> {
        view.elements.iter().copied().filter(|&e| f(e)).collect()
    };
    // The parents of the view's elements of kind `k`, sorted by id.
    let parents_of = |k: ElementKind| -> Vec<ElementId> {
        let mut out: Vec<ElementId> = Vec::new();
        for &e in &view.elements {
            if let Some(p) = parent(e).filter(|p| kind(e) == k && !out.contains(p)) {
                out.push(p);
            }
        }
        ws.sort(&mut out);
        out
    };
    let systems_of = |containers: &[ElementId]| -> Vec<ElementId> {
        let mut out: Vec<ElementId> = Vec::new();
        for p in containers.iter().filter_map(|&c| parent(c)) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    };
    // Each in turn, then a blank line if there was any.
    let lines = |w: &mut Writer, d: &mut dyn FnMut(&mut Writer, ElementId), list: &[ElementId]| {
        for &e in list {
            d(w, e);
        }
        if !list.is_empty() {
            w.blank();
        }
    };
    let scope = view.spec.scope.element();
    match view.spec.kind {
        ViewKind::Container => {
            let outside = pick(&|e| kind(e) != ElementKind::Container);
            lines(&mut w, &mut |w, e| d.element(w, ws, view, e), &outside);
            for s in parents_of(ElementKind::Container) {
                d.start_boundary(&mut w, ws, view, s);
                write_elements(&mut w, ws, view, d, &pick(&|e| kind(e) == ElementKind::Container && parent(e) == Some(s)));
                d.end_boundary(&mut w);
            }
        }
        ViewKind::Component => {
            for k in [ElementKind::Custom, ElementKind::Person, ElementKind::SoftwareSystem] {
                lines(&mut w, &mut |w, e| d.element(w, ws, view, e), &pick(&|e| kind(e) == k));
            }
            let containers = parents_of(ElementKind::Component);
            system_boundaries(&mut w, ws, view, d, (&systems_of(&containers), &containers));
        }
        ViewKind::Dynamic => match scope.map(kind) {
            Some(ElementKind::SoftwareSystem) => {
                for s in parents_of(ElementKind::Container) {
                    d.start_boundary(&mut w, ws, view, s);
                    write_elements(&mut w, ws, view, d, &pick(&|e| kind(e) == ElementKind::Container && parent(e) == Some(s)));
                    d.end_boundary(&mut w);
                }
                lines(&mut w, &mut |w, e| d.element(w, ws, view, e), &pick(&|e| parent(e).is_none()));
            }
            Some(ElementKind::Container) => {
                for k in [ElementKind::Custom, ElementKind::Person, ElementKind::SoftwareSystem] {
                    lines(&mut w, &mut |w, e| d.element(w, ws, view, e), &pick(&|e| kind(e) == k));
                }
                let containers = parents_of(ElementKind::Component);
                system_boundaries(&mut w, ws, view, d, (&systems_of(&containers), &containers));
                // Structurizr 6.2.4 writes these a second time (module
                // comment).
                let rest = pick(&|e| parent(e).is_none_or(|p| kind(p) != ElementKind::Container));
                lines(&mut w, &mut |w, e| d.element(w, ws, view, e), &rest);
            }
            _ => {
                write_elements(&mut w, ws, view, d, &view.elements);
                if !view.elements.is_empty() {
                    w.blank();
                }
            }
        },
        ViewKind::Deployment => {
            let top = pick(&|e| kind(e) == ElementKind::DeploymentNode && parent(e).is_none());
            write_elements(&mut w, ws, view, d, &top);
        }
        _ => {
            write_elements(&mut w, ws, view, d, &view.elements);
            if !view.elements.is_empty() {
                w.blank();
            }
        }
    }
    for r in &view.relationships {
        d.relationship(&mut w, ws, view, r);
    }
    if !view.relationships.is_empty() {
        w.blank();
    }
    d.footer(&mut w, ws, view);
    w
}

/// Each software system of `systems` as a boundary holding its
/// boundary containers (each holding its components) and then every
/// container of the view in it.
fn system_boundaries(
    w: &mut Writer,
    ws: &Workspace<'_>,
    view: &View<'_>,
    d: &mut impl Diagram,
    (systems, containers): (&[ElementId], &[ElementId]),
) {
    let model = ws.model;
    for &s in systems {
        d.start_boundary(w, ws, view, s);
        for &c in containers.iter().filter(|&&c| model.element(c).parent == Some(s)) {
            d.start_boundary(w, ws, view, c);
            let components: Vec<ElementId> = view
                .elements
                .iter()
                .copied()
                .filter(|&e| model.element(e).kind == ElementKind::Component && model.element(e).parent == Some(c))
                .collect();
            write_elements(w, ws, view, d, &components);
            d.end_boundary(w);
        }
        for &e in view
            .elements
            .iter()
            .filter(|&&e| model.element(e).kind == ElementKind::Container && model.element(e).parent == Some(s))
        {
            d.element(w, ws, view, e);
        }
        d.end_boundary(w);
    }
}

/// Deployment node `e` with what it holds on the view, written as
/// Structurizr's `writeElements` writes them.
fn node(w: &mut Writer, ws: &Workspace<'_>, view: &View<'_>, d: &mut impl Diagram, e: ElementId) {
    d.start_node(w, ws, view, e);
    let children: Vec<ElementId> = view
        .elements
        .iter()
        .copied()
        .filter(|&c| ws.model.element(c).parent == Some(e))
        .collect();
    write_elements(w, ws, view, d, &children);
    d.end_node(w);
}

/// What `walk` writes each element of a view as: what the `PlantUML` style
/// block and legend list.
#[derive(Debug, Default)]
struct Roles {
    boxes: Vec<ElementId>,
    boundaries: Vec<ElementId>,
    nodes: Vec<ElementId>,
    groups: Vec<String>,
}

/// The roles of the elements of `view`.
fn roles(ws: &Workspace<'_>, view: &View<'_>) -> Roles {
    struct Collect(Roles);
    impl Diagram for Collect {
        fn header(&mut self, _: &mut Writer, _: &Workspace<'_>, _: &View<'_>) {}
        fn element(&mut self, _: &mut Writer, _: &Workspace<'_>, _: &View<'_>, e: ElementId) {
            self.0.boxes.push(e);
        }
        fn start_group(&mut self, _: &mut Writer, _: &Workspace<'_>, _: &View<'_>, group: &str) {
            self.0.groups.push(group.to_owned());
        }
        fn end_group(&mut self, _: &mut Writer) {}
        fn start_boundary(&mut self, _: &mut Writer, _: &Workspace<'_>, _: &View<'_>, e: ElementId) {
            self.0.boundaries.push(e);
        }
        fn end_boundary(&mut self, _: &mut Writer) {}
        fn start_node(&mut self, _: &mut Writer, _: &Workspace<'_>, _: &View<'_>, e: ElementId) {
            self.0.nodes.push(e);
        }
        fn end_node(&mut self, _: &mut Writer) {}
        fn relationship(&mut self, _: &mut Writer, _: &Workspace<'_>, _: &View<'_>, _: &ViewRel) {}
        fn footer(&mut self, _: &mut Writer, _: &Workspace<'_>, _: &View<'_>) {}
    }
    let mut c = Collect(Roles::default());
    walk(ws, view, &mut c);
    c.0
}

/// The views in the order Structurizr's exporters walk them (custom,
/// landscape, context, container, component, dynamic, deployment), each
/// with its declaration index: a counter such as the `PlantUML` group
/// number runs in this order.
fn structurizr_order<'m>(ws: &Workspace<'m>) -> Vec<(usize, View<'m>)> {
    let rank = |k: ViewKind| match k {
        ViewKind::Custom => 0,
        ViewKind::SystemLandscape => 1,
        ViewKind::SystemContext => 2,
        ViewKind::Container => 3,
        ViewKind::Component => 4,
        ViewKind::Dynamic => 5,
        _ => 6,
    };
    let mut out: Vec<(usize, View<'m>)> = ws
        .model
        .views
        .iter()
        .enumerate()
        .filter_map(|(i, spec)| ws.view(spec).map(|v| (i, v)))
        .collect();
    out.sort_by_key(|(i, v)| (rank(v.spec.kind), *i));
    out
}
