//! `WebSequenceDiagrams`, as Structurizr 6.2.4's
//! `WebSequenceDiagramsExporter` writes it (P3-409).

use crate::model::{ElementId, ElementKind};

use super::super::structurizr::{View, Workspace, Writer};

/// `title` (the view's name and description), one `actor`/`participant`
/// line per relationship end in first-use order, one
/// `<src>-><dst>: <description>` line per step (`-->` back for a
/// response).
pub(super) fn write(ws: &Workspace<'_>, view: &View<'_>) -> String {
    let mut w = Writer::default();
    let name = ws.view_name(view.spec);
    match Workspace::description(view.spec) {
        Some(d) => w.line(&format!("title {name}\n{d}")),
        None => w.line(&format!("title {name}")),
    }
    w.blank();
    let mut participants: Vec<ElementId> = Vec::new();
    for r in &view.relationships {
        for e in [r.source, r.destination] {
            if !participants.contains(&e) {
                participants.push(e);
            }
        }
    }
    for &e in &participants {
        let name = &ws.model.element(e).name.value;
        let keyword = if ws.model.element(e).kind == ElementKind::Person { "actor" } else { "participant" };
        w.line(&format!("{keyword} <<{}>>\\n{name} as {name}", ws.kind_name(e)));
    }
    w.blank();
    for r in &view.relationships {
        let (s, d, arrow) = if r.response { (r.destination, r.source, "-->") } else { (r.source, r.destination, "->") };
        let name = |e: ElementId| &ws.model.element(e).name.value;
        w.line(&format!("{}{arrow}{}: {}", name(s), name(d), r.description));
    }
    if !view.relationships.is_empty() {
        w.blank();
    }
    w.finish()
}
