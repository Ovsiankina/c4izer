# SRS: c4rizer

_Status: POC-ready draft (single-file format decided 2026-11-15), written by the agent from
`~/Projects/c4-design/APP-PROMPT.md` and `SKILL.md`, awaiting the
owner's read. Holes are marked `TODO(srs)`. One section per C2
container. Requirements are numbered `<MOD>-<n>`, one testable sentence
each; step-4 tests are named after them._

## 1. Purpose

c4rizer is a Structurizr remake in Rust: C4 models written as text,
shown live in a fast desktop editor that reloads on every save, edited
directly on the canvas, with circuit-board lines (0/45/90 degrees, bus
lanes, minimal crossings). It replaces Structurizr and the Python
prototype in `~/Projects/c4-design/scripts/c4.py`. SVG and PNG are
export formats, as in Structurizr, not the product.

## 2. Scope and phases

c4rizer is an improved Structurizr: the same features and more, minus
the heavy server. Owner's words (2026-11-15): "its litterally an
improved version of structurizr. same features and more." The "more":
fast startup, live reload, editing text, layout and style on the
canvas, circuit-board lines (0/45/90 degrees, bus lanes, minimal
crossings).

The grammar starts from Structurizr DSL; c4rizer's additions (`at`,
`size`, view options) are extensions in the same single file. Nothing
in phase 1 may be designed so that a later phase cannot be added.

- **Phase 1 (this cycle):** the DSL subset in 4.1; system context,
  container and component views; core pipeline; CLI; desktop editor
  with every EDIT requirement; SVG and PNG export.
- **Phase 2:** Structurizr DSL breadth: system landscape and filtered
  views, groups, `!include`, `!identifiers hierarchical`, relationship
  styles, themes; an unchanged Structurizr `workspace.dsl` opens (with
  auto-placed boxes until positions are added).
- **Phase 3:** dynamic and deployment views; hand-added line vertices
  as an override on top of the router (the router stays the default);
  export to other formats.
- **Never:** collaboration, cloud, accounts, a server. Creating and
  deleting elements or relationships from the canvas: the owner said
  "it's not draw.io"; the model is written as text.

## 3. System overview

See `docs/c4/c1.svg` and `docs/c4/c2.svg`. Three crates (core, CLI,
editor), one diagram file, and the exports. The core holds every
rule and turns the file into a **scene** per view: plain data for
boxes, routed lines, labels, title and key. The editor draws the scene
on an interactive canvas; the SVG and PNG exporters write the same
scene to files. The CLI and the editor are thin shells over the core.

Reference behaviour: `~/Projects/c4-design/example/` (model, layout,
four SVGs), converted per DSL-12, is the first golden test. Its rules live in `SKILL.md`;
requirements below cite them instead of repeating them.

## 4. Modules

### 4.1 Diagram file (c4rizer DSL)

#### Responsibility

One text file per architecture is the single source of truth: elements,
relationships, styles, views, view options and box positions. The
syntax starts from Structurizr DSL because it is familiar, but c4rizer
is standalone: compatibility with Structurizr is kept only where it
costs nothing.

Proposed shape (agent draft, `TODO(srs)` until the owner reads it):

    workspace "Name" "Why it exists" {
        model {
            user = person "Worker" "..."
            system = softwareSystem "Atomanja" "..." {
                app = container "Server" "..." "Rust"
            }
            user -> app "Views tasks with" "HTTPS"
        }
        views {
            container system "c2" "Container diagram for Atomanja" {
                subtitle "The running parts of Atomanja."
                include *
                maxCrossings 0
                technology off
                bus on
                at user 220 170
                at app 620 480
                size app 280 140
            }
        }
        styles { element "Person" { shape Person  background #0b3c6f } }
    }

#### Requirements

Model

- **DSL-1** Parsing yields every `person`, `softwareSystem`,
  `container` and `component` with its id, name, description,
  technology, tags and parent.
- **DSL-2** Parsing yields every relationship with source, destination,
  description and technology, whether declared at top level or nested.
- **DSL-3** Parsing yields `element` styles with `background`, `color`
  and `shape` (`Person`, `Cylinder`, box by default).
- **DSL-4** Any construct outside the phase-1 grammar is a parse
  error naming the line, the column and the construct, and saying when
  it is planned if it is in a later phase; nothing is skipped silently.
- **DSL-5** A relationship or view entry naming an undeclared id is an
  error naming the id.

Views and positions (replaces the prototype's `layout.json`)

- **DSL-6** A view declares kind (`systemContext`, `container`,
  `component`), scope, key and title, and optionally `subtitle`,
  `include`, `exclude`, `focus`, `hide`, `technology`, `bus`,
  `labelWidth`, `maxCrossings`: every option in `SKILL.md` "Layout
  options".
- **DSL-7** A view's box positions are `at <id> <x> <y>` lines and its
  size overrides `size <id> <w> <h>` lines, one per box.
- **DSL-8** An element included in a view but without an `at` line is
  placed automatically (CORE-22); its position is written back only
  when the user moves or pins it.

Writing back

- **DSL-9** Moving one box rewrites only that box's `at` line (or adds
  one); every other byte of the file is unchanged.
- **DSL-10** Editing a name, description or relationship label rewrites
  only that string literal; every other byte is unchanged.
- **DSL-13** Changing an element's colour, text colour or shape from
  the app writes it into the `styles` block: the tag's `element` block
  when the change applies to the tag, otherwise a style for a new tag
  unique to that element; nothing else in the file changes.
- **DSL-11** Parse, then write back with no edits, gives a file
  byte-identical to the input, comments and blank lines included.

Migration

- **DSL-12** `example/workspace.dsl` + `example/layout.json` from the
  prototype are converted once into a c4rizer file that renders the
  same four views; that file is the golden-test input. `TODO(srs)`: a
  one-off converter, or a hand conversion checked in as test data.

#### Open holes

- The syntax above is a draft.
- Later phases (section 2) are rejected per DSL-4 until built.

### 4.2 Core

#### Responsibility

Parse, place, route, place labels, lint, build the scene, export it
to SVG and PNG. No UI, no file watching, no windowing; every function
is callable from a test.

#### Requirements

Views and rendering

- **CORE-1** Each view builds one scene holding a title, a subtitle,
  a generated key covering every shape, colour and line style drawn,
  and a dashed boundary around the scope's children. The canvas and the
  exporters draw only what the scene holds.
- **CORE-2** People draw as the person shape and `Cylinder` elements as
  cylinders; every box shows name, `[type: technology]` and a wrapped
  description.
- **CORE-3** Every line takes the colour of the box it points to.
- **CORE-4** With `technology: false`, no technology appears on any
  line of that view.
- **CORE-5** A relationship between two elements not both in the view
  is lifted to their nearest drawn ancestors, as the prototype does, and
  duplicate lifted lines are drawn once.

Router (`SKILL.md` "Lines")

- **CORE-6** Every line segment is at 0°, 45° or 90°, and every vertex
  sits on the 10px grid.
- **CORE-7** No line passes through a box other than its own ends.
- **CORE-8** No two lines share a segment, except bus lines joining
  into the same destination when `bus` is on, which meet at a dot.
- **CORE-9** Each line has its own port on each box; no two arrowheads
  touch the same point.
- **CORE-10** Each line carries direction chevrons along its length in
  addition to one arrowhead.
- **CORE-11** On `example/`, every view routes with crossings no higher
  than the prototype's (C1 0, C2 0, C3 1, C3-startup 0).
- **CORE-12** Lines heading the same way run as a bus with a constant
  gap. `TODO(srs)`: a measurable form of "prefers lanes".
- **CORE-25** Routing is deterministic: the same file gives the same
  scene, and the same SVG bytes, on every run and machine.
- **CORE-13** When a line cannot be routed, routing does not stop: the
  router rips up and retries; a line still unroutable is reported by
  id, never dropped silently.

Labels

- **CORE-14** Each label sits on its own line, wrapped at
  `label_width` characters, and overlaps no box, no other label and no
  other line.
- **CORE-15** A label with no clear spot is reported as a lint warning
  naming the relationship.

Lint (exit-code rules from `SKILL.md`, every **[lint]** item)

- **CORE-16** Lint reports, per view, each rule broken with the ids
  involved, and splits findings into errors and warnings exactly as the
  prototype does (`c4.py` `lint`).
- **CORE-17** Crossings above the view's `max_crossings` (default 0)
  are an error.
- **CORE-18** Box centres off the 10px grid, or closer than 60px, are
  an error.
- **CORE-19** C1 rejects containers, components and protocol words in
  box names and labels; C2 rejects components; C3 must show at least
  one component of its scope.
- **CORE-20** Each box needs name and description; containers and
  components also need technology; each relationship needs an intent
  label (labels like "uses" are rejected).
- **CORE-21** Coverage: when a container's components appear in
  several views, every relationship among them appears in at least one
  view, otherwise an error names it.

Placement

- **CORE-22** Auto-placement puts every unplaced box of a view on the
  grid with no overlap, without moving any box that already has a
  position.
- **CORE-23** Auto-placement on `example/` gives no more straight-line
  crossings than `place.py`. `TODO(srs)`: target quality.
- **CORE-26** When a view cannot meet its crossing budget after
  placement and routing, lint says so and suggests splitting the view
  (`focus`, `hide`), naming the boxes involved in the crossings.

Speed

- **CORE-24** Re-rendering one view of `example/` after a single box
  move takes under 100 ms on the owner's machine in a release build.
  `TODO(srs)`: confirm the budget; the prototype takes about 5 s a view.
- **CORE-27** Startup path: from file text to the routed scene of the
  first view of `example/` takes under 300 ms in a release build,
  measured by a test in the oracle. `TODO(srs)`: 300 ms is a target to
  confirm once measured.
- **CORE-28** Routing one view needs no other view: a caller can ask
  for one view's scene without the others being routed.

#### Interfaces (intent, not signatures)

Diagram file in, per view: a scene (boxes, routed lines, labels,
title, key, each tied to the model id it came from, so the editor can
hit-test and edit it) and a lint report (errors, warnings). Scene out:
SVG text, PNG bytes. Incremental re-route after a move is an internal
matter as long as CORE-24 holds.

#### Open holes

CORE-12, CORE-23, CORE-24 numbers.

### 4.3 CLI

#### Responsibility

Render and lint from a shell, for scripts and agents.

#### Requirements

- **CLI-1** `c4rizer render FILE -o DIR` writes one SVG per
  view and prints, per view, box count, line count, crossings against
  budget, warnings and errors, as the prototype does.
- **CLI-2** Exit code is 0 when no view has a lint error, 1 when any
  does, 2 on a parse or I/O error.
- **CLI-3** `c4rizer lint FILE` does the same checks without
  writing files.
- **CLI-4** `--png` also writes a PNG per view at a given width.
- **CLI-5** Running on `example/` writes SVGs that pass the golden
  test (CORE-11 numbers, same boxes and lines as the reference).
  `TODO(srs)`: golden = structural compare, not byte compare.

### 4.4 Editor

#### Responsibility

A native desktop window that draws the scene on an interactive
canvas, follows the diagram file live, and lets the architect arrange
boxes and edit text in place. Stack chosen by the agent on the owner's
delegation (2026-11-15): eframe/egui, file watching with `notify`, PNG
through the core exporter.

#### Requirements

- **EDIT-1** Saving the diagram file from another program updates
  the open view without a restart or a manual reload, keeping the
  selected view, zoom and pan.
- **EDIT-2** Dragging a box snaps it to the 10px grid and re-routes the
  lines while dragging.
- **EDIT-3** Releasing a dragged box writes its position to the diagram
  file (DSL-9).
- **EDIT-4** Auto-place never locks a box: any box placed automatically
  can be dragged afterwards and keeps the dragged position.
- **EDIT-5** With a box or line selected, pressing Enter or F2 edits
  its name, description or label in place; confirming writes it to the
  diagram file (DSL-10), Escape cancels.
- **EDIT-6** Lint findings are shown next to the view and update with
  each render.
- **EDIT-7** The open view can be exported to SVG and PNG.
- **EDIT-8** All views of the file are listed; picking one shows it.
- **EDIT-9** The canvas pans and zooms; zoom does not change the
  diagram, only the view of it.
- **EDIT-10** Ctrl-click or right-click on a box or line selects it
  without entering it, and shows its model details (name, type,
  description, technology, tags) and style panel. A left click on a box
  with no deeper level, or on a line, also selects it.
- **EDIT-11** A parse error in the file leaves the last good diagram on
  screen and shows the error with its line and column.
- **EDIT-12** The canvas draws from the scene only; nothing on the
  canvas is drawn that the SVG export of the same view would not show,
  apart from selection and hover marks.

Opening and starting

- **EDIT-13** The app starts with no argument and offers Open (file
  dialog) and a list of recently opened files; `c4rizer-editor FILE`
  opens a file directly.
- **EDIT-14** From launch to the first view of `example/` on screen
  takes under 500 ms cold on the owner's machine; other views route in
  the background and never delay the first one (CORE-28).
  `TODO(srs)`: confirm once the window backend is measured.

C4 levels

- **EDIT-15** A left click (press and release without moving) on a
  software system opens its container view, and on a container opens
  its component view. When
  several views are scoped to it, the first declared opens and the view
  list (EDIT-8) shows the others. When none exists, a default view
  (`include *`, every box auto-placed) opens and is written to the file
  only when the user moves a box in it. Press and drag moves the box
  instead (EDIT-2); a left click on a person, a component or an
  external system selects it (EDIT-10).
- **EDIT-16** A breadcrumb shows the path of levels opened (for example
  `c4rizer / c4rizer core`); clicking a crumb, Backspace or Alt+Left
  goes back up one level.

Editing

- **EDIT-17** Selected boxes can change colour, text colour and shape
  from a side panel; the change is written per DSL-13 and drawn at
  once.
- **EDIT-18** A box can be resized on the grid by its handles; the size
  is written as its `size` line (DSL-7).
- **EDIT-19** Undo and redo cover every change the app wrote to the
  file (moves, resizes, text, styles), at least 100 steps, for the
  session.
- **EDIT-20** Shift-click and a rubber-band drag on empty canvas
  select several boxes; they move
  together, and can be aligned (left, centre, right, top, middle,
  bottom) and distributed evenly.
- **EDIT-21** The app never creates or deletes elements or
  relationships; it changes text, layout and style only (section 2).

Working beside a text editor

- **EDIT-22** Every write is atomic (write to a temporary file, then
  rename), and the app does not reload on its own writes.
- **EDIT-23** The watcher sees saves done by rename (it watches the
  file's directory), so editors that save that way still trigger
  EDIT-1.
- **EDIT-24** Before writing, the app checks the file on disk is the
  one it last read. If it changed, the app reloads first and re-applies
  the user's action to the new text (move box X, set label of Y); if
  the target no longer exists, the action is dropped and the status bar
  says so. A text editor with unsaved changes is not visible to the app;
  the editor's own changed-on-disk prompt handles that side.
- **EDIT-25** Routing never runs on the UI thread. While a box is
  dragged, only the lines touching it are re-routed, on a worker that
  cancels stale requests; the full view is re-routed on release. The
  canvas stays at the display refresh rate during a drag.

### 4.5 Diagram exports

- **EXP-1** Exported SVG renders the same in `rsvg-convert` and a
  current browser (no scripts, no external fonts required).
- **EXP-2** PNG export of a view matches its SVG rasterized at the same
  width.

## 5. Cross-cutting

### 5.1 Errors

Parse and I/O errors carry file, line and column where they
exist, and a sentence saying what to change. Lint findings are data,
not errors.

### 5.2 Performance

Startup first (the owner's main complaint about Structurizr):
CORE-27, CORE-28, EDIT-14. Then re-render after a change: CORE-24,
EDIT-25. Routing is the hot path. A disk cache of routed scenes is
not a requirement: it is added only if CORE-27 cannot be met
otherwise, since it would add a second file beside the single diagram
file.

### 5.3 Safety and `unsafe`

No `unsafe` expected.

### 5.4 Test data

No personal names in examples or test data; people are roles.

## 6. Glossary

- **Bus**: lines heading the same way, side by side at a constant gap.
- **Port**: the point on a box edge where one line starts or ends.
- **Lifting**: drawing a relationship between two hidden elements as a
  line between their visible ancestors.
- **Golden test**: rendering `example/` and comparing with the
  reference SVGs.
