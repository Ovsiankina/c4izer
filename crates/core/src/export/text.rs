//! Structurizr's text export formats (SRS 4.7, row exporters): PlantUML
//! in both flavours, Mermaid, WebSequenceDiagrams, DOT and Ilograph,
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

use crate::model::Model;
use crate::pipeline::AllViews;

use super::Flavour;

/// The PlantUML files of every drawable view: `structurizr-<key>.puml`,
/// and with `Flavour::Structurizr` also `structurizr-<key>-key.puml`
/// (P3-404). Image and filtered views give no file (P3-405). Serves
/// P3-404, P3-406, P3-407.
pub fn plantuml(model: &Model, all: &AllViews, flavour: Flavour) -> Vec<(String, String)> {
    let _ = (model, all, flavour);
    todo!()
}

/// The Mermaid file `structurizr-<key>.mmd` of every drawable view, node
/// ids being Structurizr's numeric element ids (P3-408). Serves P3-404,
/// P3-408.
pub fn mermaid(model: &Model, all: &AllViews) -> Vec<(String, String)> {
    let _ = (model, all);
    todo!()
}

/// The WebSequenceDiagrams file `structurizr-<key>.wsd` of every dynamic
/// view (P3-409); other views give no file (P3-405). Serves P3-404,
/// P3-409.
pub fn websequencediagrams(model: &Model, all: &AllViews) -> Vec<(String, String)> {
    let _ = (model, all);
    todo!()
}

/// The Graphviz file `structurizr-<key>.dot` of every drawable view, as
/// `structurizr-export` 6.2.4 `DOTExporter` writes it (P3-410); with
/// `positions`, each node also gets `pos="<x>,<y>!"` from its box in the
/// view's scene (P3-411). Serves P3-404, P3-410, P3-411.
pub fn dot(model: &Model, all: &AllViews, positions: bool) -> Vec<(String, String)> {
    let _ = (model, all, positions);
    todo!()
}

/// The one Ilograph file `workspace.idl.yaml` of the whole workspace, as
/// `structurizr-export` 6.2.4 `IlographExporter` writes it (P3-412).
/// Serves P3-412.
pub fn ilograph(model: &Model, all: &AllViews) -> Vec<(String, String)> {
    let _ = (model, all);
    todo!()
}
