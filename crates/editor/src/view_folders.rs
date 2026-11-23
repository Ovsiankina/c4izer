//! C-level folders of the view list (SRS 4.9 P5-600..P5-607, issues #10
//! and #11): which folder a view goes in, the folder list of one parse,
//! the fold state per file, the walk Previous/Next view take, and the
//! recap card's texts.
//!
//! Depends on: `state` (`State`, `ViewEntry`, `Effect`), `view_map`
//! (`level_of`), `worker` (`ViewRef`), `c4rizer-core` (`model`).

// Shape: `folder_of` is the one place that decides a view's folder
// (design section 7): the list, the walk and the tests all go through
// it. `folders` groups a view list (`State::views`) into folders, pure;
// `FolderCache` keeps the result with the parse and view it came from
// (`Arc::ptr_eq`, as `ViewMapPanel::sync`), so the left panel rebuilds
// it on a model or view change, never per frame. Fold state is plain data
// in `State::ui` (`UiState::folded`), changed only by
// `Command::ToggleFolder` through `State::handle`.

use std::collections::BTreeSet;
use std::sync::Arc;

use c4rizer_core::model::{ElementKind, Model, Parsed, ViewKind, ViewSpec};

use crate::state::{Effect, State, ViewEntry};
use crate::view_map::level_of;
use crate::worker::ViewRef;

/// A folder of the view list (P5-600). PENDING SIGN-OFF: new pub item
/// (owner request 2026-11-23, issue #11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ViewFolder {
    /// System landscape and system context views.
    C1,
    /// Container views.
    C2,
    /// Component views.
    C3,
    /// Image views scoped to a component.
    C4,
    /// Dynamic, deployment, custom and other image views.
    Other,
}

impl ViewFolder {
    /// Every folder in list order (P5-601). PENDING SIGN-OFF.
    pub const ALL: [ViewFolder; 5] = [ViewFolder::C1, ViewFolder::C2, ViewFolder::C3, ViewFolder::C4, ViewFolder::Other];

    /// The folder's key: `C1`..`C4`, `Other`; also its name in the UI
    /// state file (P5-604). PENDING SIGN-OFF.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            ViewFolder::C1 => "C1",
            ViewFolder::C2 => "C2",
            ViewFolder::C3 => "C3",
            ViewFolder::C4 => "C4",
            ViewFolder::Other => "Other",
        }
    }

    /// The folder named `key` (`ViewFolder::key`), ASCII case ignored.
    /// PENDING SIGN-OFF.
    #[must_use]
    pub fn from_key(key: &str) -> Option<ViewFolder> {
        ViewFolder::ALL.into_iter().find(|f| f.key().eq_ignore_ascii_case(key))
    }

    /// The header's label (P5-602). PENDING SIGN-OFF.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            ViewFolder::C1 => "Context",
            ViewFolder::C2 => "Containers",
            ViewFolder::C3 => "Components",
            ViewFolder::C4 => "Code",
            ViewFolder::Other => "Other",
        }
    }

    /// The recap card's title (P5-607): `C1 · System context`.
    /// PENDING SIGN-OFF.
    #[must_use]
    pub const fn recap_title(self) -> &'static str {
        match self {
            ViewFolder::C1 => "C1 · System context",
            ViewFolder::C2 => "C2 · Container",
            ViewFolder::C3 => "C3 · Component",
            ViewFolder::C4 => "C4 · Code",
            ViewFolder::Other => "Other",
        }
    }

    /// The recap card's sentence, verbatim from issue #10 (P5-607).
    /// PENDING SIGN-OFF.
    #[must_use]
    pub const fn recap(self) -> &'static str {
        match self {
            ViewFolder::C1 => "The starting point; how the software system in scope fits into the world around it.",
            ViewFolder::C2 => "Zooms into the system; the applications and data stores inside it.",
            ViewFolder::C3 => "Zooms into one container; the components inside it.",
            ViewFolder::C4 => "Zooms into one component; how it is implemented at the code level (e.g. UML class diagram).",
            ViewFolder::Other => "Supplementary diagrams: dynamic, deployment, custom and image views.",
        }
    }
}

/// The folder of a declared view (P5-600), on `view_map::level_of`'s
/// levels: landscape and context C1, container C2, component C3; an image
/// view of a component C4; dynamic, deployment, custom and other image
/// views Other; a filtered view its base's.
// DECISION: dynamic and deployment views go in Other even though the map
// puts them on a C row (`level_of`): the design's table, so a C folder
// holds only the plain view of its level.
fn folder_of_spec(model: &Model, spec: &ViewSpec) -> ViewFolder {
    match spec.kind {
        ViewKind::Filtered => model
            .base_view(spec)
            .filter(|b| b.kind != ViewKind::Filtered)
            .map_or(ViewFolder::Other, |base| folder_of_spec(model, base)),
        ViewKind::Image => {
            if spec.scope.element().is_some_and(|e| model.element(e).kind == ElementKind::Component) {
                ViewFolder::C4
            } else {
                ViewFolder::Other
            }
        }
        ViewKind::Dynamic | ViewKind::Deployment | ViewKind::Custom => ViewFolder::Other,
        _ => match level_of(model, spec).0 {
            0 | 1 => ViewFolder::C1,
            2 => ViewFolder::C2,
            3 => ViewFolder::C3,
            _ => ViewFolder::Other,
        },
    }
}

/// The folder of `view` in `model` (P5-600): the one function that
/// decides it. An EDIT-15 default view that is not written goes by its
/// kind. PENDING SIGN-OFF: new pub item.
#[must_use]
pub fn folder_of(model: &Model, view: &ViewRef) -> ViewFolder {
    match view {
        ViewRef::Declared(key) => model.view(key).map_or(ViewFolder::Other, |spec| folder_of_spec(model, spec)),
        // DECISION: a default view is a context, container or component
        // view of its scope; its kind alone gives the folder.
        ViewRef::Default { kind, .. } => match kind {
            ViewKind::SystemLandscape | ViewKind::SystemContext => ViewFolder::C1,
            ViewKind::Container => ViewFolder::C2,
            ViewKind::Component => ViewFolder::C3,
            _ => ViewFolder::Other,
        },
    }
}

/// One folder of the view list with its views in list order (P5-601).
/// PENDING SIGN-OFF: new pub item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// The folder.
    pub folder: ViewFolder,
    /// Its views, in view-list order (P2-716); never empty.
    pub views: Vec<ViewEntry>,
}

impl Folder {
    /// True when the view on screen is in this folder (P5-603's dot).
    #[must_use]
    pub fn holds_current(&self) -> bool {
        self.views.iter().any(|e| e.current)
    }
}

/// `entries` (a view list, `State::views`) grouped into their folders
/// (P5-600, P5-601): only non-empty folders, in `ViewFolder::ALL` order,
/// each keeping the list's order. PENDING SIGN-OFF: new pub item.
#[must_use]
pub fn folders(model: &Model, entries: Vec<ViewEntry>) -> Vec<Folder> {
    let mut out: Vec<Folder> = ViewFolder::ALL.iter().map(|&folder| Folder { folder, views: Vec::new() }).collect();
    for entry in entries {
        let folder = folder_of(model, &entry.view);
        // `ALL` is in enum order, so the folder's index is its place.
        if let Some(slot) = out.iter_mut().find(|f| f.folder == folder) {
            slot.views.push(entry);
        }
    }
    out.retain(|f| !f.views.is_empty());
    out
}

/// The views in the order the list displays them, less the views of the
/// `folded` folders (P5-605); the view on screen keeps its place even
/// in a folded folder, so Previous/Next start from it.
// DECISION: the current view stays in the walk when its folder is
// folded (it was opened from the map or quick navigation): otherwise
// Previous/Next would have no place to start from and jump to the top.
#[must_use]
pub fn walk(folders: &[Folder], folded: &BTreeSet<ViewFolder>) -> Vec<ViewEntry> {
    folders
        .iter()
        .flat_map(|f| {
            let open = !folded.contains(&f.folder);
            f.views.iter().filter(move |e| open || e.current)
        })
        .cloned()
        .collect()
}

impl State {
    /// The folders folded in the open file (P5-604); empty with no file.
    /// PENDING SIGN-OFF: new pub item.
    #[must_use]
    pub fn folded(&self) -> BTreeSet<ViewFolder> {
        self.doc
            .as_ref()
            .and_then(|d| self.ui.folded.get(&d.path))
            .cloned()
            .unwrap_or_default()
    }

    /// True when `folder` is folded in the open file (P5-603); no
    /// allocation, so the list may ask per frame.
    #[must_use]
    pub(crate) fn is_folded(&self, folder: ViewFolder) -> bool {
        self.doc
            .as_ref()
            .and_then(|d| self.ui.folded.get(&d.path))
            .is_some_and(|set| set.contains(&folder))
    }

    /// The view list in folders (P5-600, P5-601). Empty with no parse.
    /// PENDING SIGN-OFF: new pub item.
    #[must_use]
    pub fn view_folders(&self) -> Vec<Folder> {
        match self.model() {
            Some(model) => folders(model, self.views()),
            None => Vec::new(),
        }
    }

    /// The views Previous/Next view walk (P5-605): `walk` of the folders.
    #[must_use]
    pub(crate) fn walk_views(&self) -> Vec<ViewEntry> {
        walk(&self.view_folders(), &self.folded())
    }

    /// Folds `folder` of the open file, or unfolds it (P5-603), and saves
    /// the UI state (P5-604). Nothing with no file.
    pub(crate) fn toggle_folder(&mut self, folder: ViewFolder) -> Vec<Effect> {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else {
            return Vec::new();
        };
        let set = self.ui.folded.entry(path.clone()).or_default();
        if !set.remove(&folder) {
            set.insert(folder);
        }
        if set.is_empty() {
            self.ui.folded.remove(&path);
        }
        vec![Effect::SaveUiState(self.ui.clone())]
    }
}

/// The folder list of the last parse and view (P5-606), with the key
/// column's width measured once for it.
#[derive(Debug, Clone, Default)]
pub struct FolderCache {
    /// The parse and view the list was built for.
    built_for: Option<(Arc<Parsed>, Option<ViewRef>)>,
    /// The folders.
    folders: Arc<Vec<Folder>>,
    /// The key column's width, px (`panels::key_column`).
    pub key_w: f32,
    /// How many lists were built (P5-606).
    pub builds: usize,
}

impl FolderCache {
    /// The folders of the last `sync`.
    #[must_use]
    pub fn folders(&self) -> Arc<Vec<Folder>> {
        Arc::clone(&self.folders)
    }

    /// The folders of `state`, rebuilt only when its parse or view
    /// changed since the last call (P5-606); `measure` gives the key
    /// column's width for a fresh list.
    pub fn sync(&mut self, state: &State, measure: impl FnOnce(&[Folder]) -> f32) -> Arc<Vec<Folder>> {
        let parsed = state.doc.as_ref().and_then(|d| d.parsed.clone());
        let fresh = match (&self.built_for, &parsed) {
            (Some((p, view)), Some(now)) => !(Arc::ptr_eq(p, now) && view.as_ref() == state.view.as_ref()),
            (None, None) => false,
            _ => true,
        };
        if fresh {
            self.folders = Arc::new(state.view_folders());
            self.key_w = measure(&self.folders);
            self.builds += 1;
            self.built_for = parsed.map(|p| (p, state.view.clone()));
        }
        Arc::clone(&self.folders)
    }
}
