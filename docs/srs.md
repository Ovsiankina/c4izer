# SRS: c4rizer

_Status: POC-ready draft (single-file format decided 2026-11-15; SRS
audit folded in 2026-11-15), written by the agent from
`~/Projects/c4-design/APP-PROMPT.md`, `SKILL.md` and `scripts/c4.py`,
awaiting the owner's read. Holes are marked `TODO(srs)`. One section
per C2 container, then one per later phase. Requirements are numbered
`<MOD>-<n>`, one testable sentence each; step-4 tests are named after
them. Ids are never renumbered; a new requirement takes the next free
number in its family._

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
- **Phase 2 (section 4.6):** Structurizr DSL breadth: the full grammar
  parsed (deployment and dynamic constructs included), system landscape
  and filtered views, groups, `!include`, `!identifiers hierarchical`,
  every style property and shape, relationship styles, themes; every
  valid Structurizr `workspace.dsl` opens unchanged (boxes auto-placed
  until positions are added); `c4rizer import-layout`.
- **Phase 3 (section 4.7):** dynamic and deployment views; hand-added
  line vertices as an override on top of the router (the router stays
  the default); the further export formats the parity table gives
  phase 3.
- **Never:** collaboration, cloud, accounts, a server. Creating and
  deleting elements or relationships from the canvas: the owner said
  "it's not draw.io"; the model is written as text.

The run is complete only when every requirement of every phase is met.

Every Structurizr feature has a row in the parity table below with
phase 1, 2, 3 or never; a feature with no row is a defect of this
document.

### Parity with Structurizr

Every Structurizr feature has a phase; nothing is left unplaced. Built from the Structurizr reference on 2026-11-15; sources at the end of the table.

| area | feature | phase | note |
|---|---|---|---|
| DSL keyword | `workspace [name] [description]` | 1 | |
| DSL keyword | `workspace extends <file\|url>` | 2 | Local file first; the URL form needs a network fetch |
| DSL keyword | `model` | 1 | |
| DSL keyword | `views` | 1 | |
| DSL keyword | `styles` | 1 | |
| DSL keyword | `person <name> [description] [tags]` | 1 | DSL-1 |
| DSL keyword | `softwareSystem <name> [description] [tags]` | 1 | DSL-1 |
| DSL keyword | `container <name> [description] [technology] [tags]` | 1 | DSL-1 |
| DSL keyword | `component <name> [description] [technology] [tags]` | 1 | DSL-1 |
| DSL keyword | `<id> = <element>` identifier assignment | 1 | Used in the SRS 4.1 example |
| DSL keyword | `element <name> [metadata] [description] [tags]` (custom element) | 2 | Custom elements |
| DSL keyword | `group <name>` (nestable, also inside a component block) | 2 | Styled with tags `Group` and `Group:Name` |
| DSL keyword | `archetypes { name = <type> {...}; name = -> {...} }` | 2 | User-defined element and relationship types, with inheritance |
| DSL keyword | `->` relationship `<src> -> <dest> [desc] [tech] [tags]` | 1 | DSL-2 |
| DSL keyword | Nested `-> <dest>` (source is the enclosing element) | 1 | DSL-2 "nested" |
| DSL keyword | `this` (the element in scope) | 1 | DSL-2 (`this` as relationship source) |
| DSL keyword | element declared without `<id> =` | 2 | DSL-4 error naming phase 2 in phase 1 (DSL-7) |
| DSL keyword | named relationship `r = a -> b` | 2 | DSL-4 error naming phase 2 in phase 1 (DSL-7) |
| DSL keyword | line continuation `\` at end of line | 2 | lexed in phase 1, rejected per DSL-4 |
| DSL keyword | `"""` text blocks | 2 | lexed in phase 1, rejected per DSL-4; value rules (indent, escapes) as the Structurizr reference gives them |
| DSL keyword | `--<archetype>->` relationship | 2 | Needs archetypes |
| DSL keyword | `-/>` remove relationship (deployment environment) | 3 | Deployment |
| DSL keyword | `deploymentEnvironment <name>` | 3 | |
| DSL keyword | `deploymentGroup <name>` | 3 | |
| DSL keyword | `deploymentNode <name> [desc] [tech] [tags] [instances]` | 3 | |
| DSL keyword | `infrastructureNode <name> [desc] [tech] [tags]` | 3 | |
| DSL keyword | `softwareSystemInstance <id> [deploymentGroups] [tags]` | 3 | |
| DSL keyword | `containerInstance <id> [deploymentGroups] [tags]` | 3 | |
| DSL keyword | `instanceOf <id> [deploymentGroups] [tags]` | 3 | Alias of the two instance keywords |
| DSL keyword | `healthCheck <name> <url> [interval] [timeout]` | never | Parsed (P2-1), no effect (no polling) |
| DSL keyword | `configuration { scope; visibility; users; properties }` | 2 | Only `scope` and `properties` matter; see workspace settings |
| directive | `!include <file\|directory\|url>` | 2 | Local paths must stay under the parent dir; a URL must be HTTPS |
| directive | `!identifiers <hierarchical\|flat>` | 2 | |
| directive | `!impliedRelationships <true\|false\|fqcn>` | 2 | true/false built in; a Java strategy FQCN is parsed, no effect |
| directive | `!docs <path> [fqcn]` | 2 | Attach Markdown/AsciiDoc; viewer is a UI row (phase 3) |
| directive | `!adrs <path> [type\|fqn]` (and `!decisions`) | 2 | Same as `!docs` |
| directive | `!element` / `<id> = !element <name>` | 2 | |
| directive | `!elements <expression> {...}` | 2 | Bulk operations |
| directive | `!relationship` / `<id> = !relationship <name>` | 2 | |
| directive | `!relationships <expression> {...}` | 2 | |
| directive | `!extend`, `!ref` (deprecated) | 2 | Aliases of `!element` / `!relationship` |
| directive | `!components` (Java component finder) | never | Needs the JVM; parsed, no effect |
| directive | `!script <groovy\|kotlin\|ruby\|javascript>` / `!script <file>` | never | Needs the JVM; parsed, no effect, with a warning |
| directive | `!plugin <fqcn>` | never | Java class in a JAR; parsed, no effect, with a warning |
| body statement | `description` | 1 | Element block child |
| body statement | `technology` | 1 | Element block child |
| body statement | `tags "a,b"` | 1 | Element and relationship child (DSL-1, DSL-2) |
| body statement | `tag "<name>"` | 2 | Not in the 4.1 keyword table |
| body statement | `url` | 2 | Element and relationship |
| body statement | `properties { name value }` | 2 | Workspace, element, relationship, view, style |
| body statement | `perspectives` (inline form or `perspective <name> {description, value, url}` block) | 2 | |
| body statement | `instances <n\|range>` | 3 | Deployment nodes |
| body statement | `name` / `description` (workspace children) | 2 | |
| element style property | `shape` | 1 | DSL-3 |
| element style property | `background` | 1 | DSL-3 |
| element style property | `color` | 1 | DSL-3 |
| element style property | `colour` | 2 | Alias of `color` |
| element style property | `icon <file\|url>` | 2 | |
| element style property | `width`, `height` | 2 | Per-box `size` (DSL-7) is phase 1; this is per tag |
| element style property | `stroke` | 2 | |
| element style property | `strokeWidth <1-10>` | 2 | |
| element style property | `fontSize` | 2 | |
| element style property | `border solid\|dashed\|dotted` | 2 | |
| element style property | `opacity <0-100>` | 2 | |
| element style property | `metadata <true\|false>` | 2 | |
| element style property | `description <true\|false>` | 2 | |
| element style property | `properties {}` | 2 | |
| element style property | Styling tags `Group`, `Group:Name`, `Boundary`, `Boundary:SoftwareSystem`, `Boundary:Container`, `Boundary:Enterprise` | 2 | Enterprise boundary is deprecated |
| shape | Box | 1 | Default (DSL-3) |
| shape | Person | 1 | DSL-3 |
| shape | Cylinder | 1 | DSL-3 |
| shape | RoundedBox | 2 | Not in DSL-3 |
| shape | Circle, Ellipse, Hexagon, Diamond | 2 | |
| shape | Bucket, Pipe | 2 | |
| shape | Robot | 2 | |
| shape | Folder, WebBrowser, Window, Terminal, Shell | 2 | |
| shape | MobileDevicePortrait, MobileDeviceLandscape | 2 | |
| shape | Component | 2 | |
| relationship style property | `thickness` | 2 | |
| relationship style property | `color` / `colour` | 2 | |
| relationship style property | `style solid\|dashed\|dotted` | 2 | |
| relationship style property | `routing Direct\|Orthogonal\|Curved` | 2 | Maps onto the c4rizer router, which stays the default |
| relationship style property | `jump <true\|false>` | 2 | Line jumps at crossings |
| relationship style property | `fontSize` | 2 | |
| relationship style property | `width` | 2 | Label width |
| relationship style property | `position <0-100>` | 2 | Label position along the line |
| relationship style property | `opacity` | 2 | |
| relationship style property | `properties {}` | 2 | |
| view type | `systemLandscape [key] [desc]` | 2 | |
| view type | `systemContext <system> [key] [desc]` | 1 | |
| view type | `container <system> [key] [desc]` | 1 | |
| view type | `component <container> [key] [desc]` | 1 | |
| view type | `filtered <baseKey> <include\|exclude> <tags> [key] [desc]` | 2 | |
| view type | `custom [key] [title] [desc]` | 2 | Custom elements only |
| view type | `image <*\|element> [key]` with `image <file\|url>` source | 2 | Static PNG/SVG "if feasible" |
| view type | `image` view with `plantuml` / `mermaid` source | never | Needs `plantuml.url` / `mermaid.url` remote renderers; parsed, placeholder shown |
| view type | `image` view with `kroki <format>` source | never | Kroki remote rendering; parsed, no effect |
| view type | `dynamic <*\|system\|container> [key] [desc]` | 3 | |
| view type | `deployment <*\|system> <env> [key] [desc]` | 3 | |
| view statement | `include *` / `include <id>` | 1 | DSL-6 |
| view statement | `include *?` (reluctant wildcard) | 2 | |
| view statement | `exclude <id>` | 1 | DSL-6 |
| view statement | `exclude "<src> -> <dest>"` relationship patterns | 2 | |
| view statement | Element expressions `->x`, `x->`, `->x->`, `element==...` | 2 | |
| view statement | `element.type==`, `.parent==`, `.tag==`/`!=`, `.technology==`/`!=`, `.properties[n]==`, `.group==` | 2 | |
| view statement | Relationship expressions `*->*`, `relationship.tag==`/`!=`, `.properties[]`, `.source==`, `.destination==` | 2 | |
| view statement | `&&` / `\|\|` combinators | 2 | |
| view statement | `autoLayout [tb\|bt\|lr\|rl] [rankSep] [nodeSep]` | 1 | DSL-6 accepts and keeps it; boxes auto-placed (CORE-22, DSL-8); the arguments have no effect |
| view statement | `default` | 2 | |
| view statement | `title <title>` | 1 | DSL-6: drawn as the view title |
| view statement | `description` | 1 | DSL-6 |
| view statement | `properties {}` (view) | 2 | |
| view statement | `animation { ids per line }` | 3 | |
| view statement | Dynamic `[order:] <src> -> <dest> [desc] [tech]` | 3 | |
| view statement | Dynamic parallel `{ }` blocks / repeated step numbers | 3 | |
| workspace setting | `styles { light {...} dark {...} }` | 2 | Light and dark mode styles |
| workspace setting | `theme <name\|url\|file>` / `themes ...` | 2 | Prebuilt AWS/Azure/GCP/OCI/Kubernetes themes; remote URL needs a fetch, so prefer bundled files |
| workspace setting | `terminology` (person, softwareSystem, container, component, deploymentNode, infrastructureNode, relationship, metadata symbols) | 2 | Deployment keys only matter once phase 3 lands |
| workspace setting | `configuration scope <landscape\|softwaresystem\|none>` | 2 | Validation rule |
| workspace setting | `configuration visibility`, `users {}` | never | Server and accounts; parsed, no effect |
| workspace setting | View-set properties `structurizr.locale`, `.timezone`, `.sort`, `.tooltips`, `.title`, `.description`, `.metadata`, `.groups` | 2 | |
| workspace setting | Image view-set properties `plantuml.url`/`.format`, `mermaid.url`/`.format`, `kroki.url`/`.format` | never | Remote rendering; parsed, no effect |
| workspace setting | `structurizr.dsl` / `structurizr.dsl.source` properties | 3 | Only matter for JSON export |
| export format | SVG | 1 | CLI-1, EXP-1 |
| export format | PNG | 1 | CLI-4, EXP-2 |
| export format | PNG/SVG `-mode dark` | 2 | Follows light/dark styles |
| export format | PNG/SVG `-animation true` (one image per step) | 3 | |
| export format | `plantuml` / `plantuml/structurizr` | 3 | |
| export format | `plantuml/c4plantuml` | 3 | |
| export format | `mermaid` | 3 | |
| export format | `websequencediagrams` | 3 | Dynamic views only |
| export format | `static` (HTML site) | 3 | |
| export format | `json` (Structurizr workspace JSON) | 3 | |
| export format | `theme` (JSON theme from workspace styles) | 3 | |
| export format | Custom exporter by FQCN (`plugins` JAR) | never | Needs the JVM |
| export format | DOT (Graphviz) | 3 | Not a Structurizr format; listed in the c4rizer phase-3 brief |
| CLI command | `c4rizer render` | 1 | CLI-1; covers `export -format svg/png` |
| CLI command | `c4rizer lint` | 1 | CLI-3 |
| CLI command | `export` (non-SVG/PNG formats) | 3 | |
| CLI command | `validate -workspace` | 2 | |
| CLI command | `inspect -workspace [-severity]` | 2 | Exit code = violation count; `-inspector` Java class: never |
| CLI command | `list -workspace` | 2 | Lists elements |
| CLI command | `merge -workspace -layout [-view] -output` | 3 | Layout from workspace JSON |
| CLI command | `generate system-landscape -input -output` | 3 | Merges many workspaces, JSON input |
| CLI command | `version`, `help` | 2 | hand-written, like the rest of the CLI (no `clap`); no phase 1 requirement names them |
| CLI command | `local` (formerly Lite) | 1 | Replaced by `c4rizer-editor` (EDIT-13) |
| CLI command | `playground` | never | Replaced by the editor |
| CLI command | `server` | never | |
| CLI command | `push`, `pull`, `branches`, `create`, `delete`, `lock`, `unlock`, `regenerate-apikey` | never | Server-bound |
| UI feature | Drag elements to move | 1 | EDIT-2 |
| UI feature | Save layout | 1 | EDIT-3, DSL-9 |
| UI feature | Automatic layout for views with no positions | 1 | CORE-22 |
| UI feature | Auto-layout modal: run layout again on demand | 2 | |
| UI feature | Bring all elements back onto the canvas (magnet) | 2 | |
| UI feature | Remove all relationship vertices | 3 | Goes with vertices |
| UI feature | Canvas size decrease/auto/increase | 2 | Paper size for export |
| UI feature | Multi-select (Shift/Alt-click, lasso) | 1 | EDIT-20 |
| UI feature | Align left/centre/right/top/middle/bottom, distribute horizontally/vertically | 1 | EDIT-20 |
| UI feature | Keyboard shortcuts l/c/r/t/m/b/h/v for align and distribute | 2 | |
| UI feature | Centre diagram or selection | 2 | |
| UI feature | Add, move, remove line vertices | 3 | Override on top of the router |
| UI feature | Move a relationship label with Up/Down | 2 | Writes `position` |
| UI feature | Cycle routing with `r`, toggle jumps with `j` | 2 | |
| UI feature | Diagram navigator / view list | 1 | EDIT-8 |
| UI feature | Open the next level of detail from an element | 1 | EDIT-15 (single click, not double-click) |
| UI feature | Keyboard prev/next/back/quick navigation between diagrams | 2 | Phase 1 has breadcrumb back only (EDIT-16) |
| UI feature | Open an element/relationship URL (Alt+double-click) | 2 | Needs `url` |
| UI feature | Zoom in/out | 1 | EDIT-9 |
| UI feature | Fit to width/height/content | 2 | |
| UI feature | Fullscreen | 2 | |
| UI feature | Diagram key / legend generated from styles | 1 | CORE-1 |
| UI feature | Toggle element/relationship descriptions and metadata | 2 | |
| UI feature | Tooltips (toggle, sticky) | 2 | |
| UI feature | Filter by tags | 2 | |
| UI feature | Perspectives view (`p` key) | 2 | |
| UI feature | Light/dark diagram mode | 2 | |
| UI feature | Animation step forward/back, start/stop | 3 | |
| UI feature | Export PNG/SVG from the UI | 1 | EDIT-7 |
| UI feature | Graph exploration | 3 | |
| UI feature | Documentation and decisions viewer | 3 | Content parsed in phase 2 |
| UI feature | Refresh on file change | 1 | Structurizr local needs a browser refresh; c4rizer live-reloads (EDIT-1) |
| UI feature | Edit gated by workspace write access | never | Accounts |
| UI feature | iframe embed, image embed, static-site embed | never | Hosting and server |
| UI feature | Workspace sharing, versions, branches, multi-user | never | Server and collaboration |

Sources:

- https://docs.structurizr.com/dsl/language
- https://docs.structurizr.com/ (navigation index)
- https://docs.structurizr.com/dsl/expressions
- https://docs.structurizr.com/dsl/archetypes
- https://docs.structurizr.com/dsl/includes
- https://docs.structurizr.com/dsl/implied-relationships
- https://docs.structurizr.com/dsl/workspace-extension
- https://docs.structurizr.com/dsl/scripts
- https://docs.structurizr.com/dsl/plugins
- https://docs.structurizr.com/dsl/cookbook/dynamic-view-parallel/
- https://docs.structurizr.com/dsl/cookbook/image-view/
- https://docs.structurizr.com/commands
- https://docs.structurizr.com/export
- https://docs.structurizr.com/export/png-and-svg
- https://docs.structurizr.com/export/static-site
- https://docs.structurizr.com/validate
- https://docs.structurizr.com/inspect
- https://docs.structurizr.com/merge
- https://docs.structurizr.com/list
- https://docs.structurizr.com/generate
- https://docs.structurizr.com/local
- https://docs.structurizr.com/local/diagrams
- https://docs.structurizr.com/server/diagrams/editor
- https://docs.structurizr.com/server/diagrams/viewer
- https://docs.structurizr.com/server/diagrams/themes
- https://docs.structurizr.com/server/diagrams/perspectives
- https://docs.structurizr.com/server/diagrams/customisation
- https://docs.structurizr.com/server/diagrams/notation

## 3. System overview

See `docs/c4/c1.svg` and `docs/c4/c2.svg`. Three crates (core, CLI,
editor), one diagram file, and the exports. The core holds every
rule and turns the file into a **scene** per view: plain data for
boxes, routed lines, labels, title and key. The editor draws the scene
on an interactive canvas; the SVG and PNG exporters write the same
scene to files. The CLI and the editor are thin shells over the core.

Reference behaviour: `~/Projects/c4-design/example/` (model, layout,
four SVGs), copied into `tests/data/example/` and converted per DSL-12,
is the first golden test. The prototype rules c4rizer keeps are
written out below (keyword table in 4.1, router rules and lint table in
4.2); this document, not the prototype, is the contract.

## 4. Modules

### 4.1 Diagram file (c4rizer DSL)

#### Responsibility

One text file per architecture is the single source of truth: elements,
relationships, styles, views, view options and box positions. The
syntax is Structurizr DSL: every valid Structurizr DSL file is a valid
c4rizer file by the end of phase 2; c4rizer adds `at`, `size` and view
options. Phase 1 parses the subset in the keyword table below.

Shape:

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
            styles {
                element "Person" {
                    shape Person
                    background #0b3c6f
                }
            }
        }
    }

#### Phase 1 keywords

Keywords are case-insensitive. Comments are `//` and `#` to the end of
the line, and `/* ... */`. Strings are in double quotes; `\"` and `\\`
are the only escapes. `[x]` means optional; `<id>` may be a dotted path
(DSL-7).

| keyword | arguments | default | meaning |
|---|---|---|---|
| `workspace` | `["name"] ["description"] { }` | — | root block |
| `model` | `{ }` | — | holds elements and relationships |
| `person` | `<id> = person "name" ["description"] ["tags"] [{ }]` | — | a person (a role, never a name) |
| `softwareSystem` | `<id> = softwareSystem "name" ["description"] ["tags"] [{ }]` | — | a software system; its block holds containers |
| `container` | `<id> = container "name" ["description"] ["technology"] ["tags"] [{ }]` | — | a container; its block holds components |
| `component` | `<id> = component "name" ["description"] ["technology"] ["tags"] [{ }]` | — | a component |
| `description` | `"text"` in an element block | positional value | sets the element's description |
| `technology` | `"text"` in a container or component block | positional value | sets the element's technology |
| `tags` | `"a,b"` in an element or relationship block | none | adds tags (comma-separated, trimmed) |
| `->` | `<src> -> <dst> ["description"] ["technology"] ["tags"] [{ }]` | — | a relationship; inside an element block the source may be `this` or left out (`-> <dst> ...`), meaning the enclosing element |
| `views` | `{ }` | — | holds views and `styles` |
| `systemContext` | `<system> ["key"] ["description"] { }` | key: generated | C1 view of a software system |
| `container` (in `views`) | `<system> ["key"] ["description"] { }` | key: generated | C2 view of a software system |
| `component` (in `views`) | `<container> ["key"] ["description"] { }` | key: generated | C3 view of a container |
| `include` | `*` or `<id>...` | nothing included | adds elements; `*` selects what Structurizr selects for the view kind: C1 the scope system and the people and systems linked to it; C2 the scope's containers and the people and systems linked to them; C3 the scope's components and the elements linked to them |
| `exclude` | `<id>...` | none | removes elements included above |
| `autoLayout` | `[tb\|bt\|lr\|rl] [rankSep] [nodeSep]` | — | kept byte for byte; boxes without an `at` line are auto-placed (CORE-22); `at` lines win; the arguments have no effect |
| `title` | `"text"` | the view description | drawn as the view title |
| `description` | `"text"` in a view | positional value | the view description |
| `subtitle` | `"text"` | empty | c4rizer extension: drawn under the title |
| `technology` | `on\|off` | `on` | c4rizer extension: `off` hides technology on every line of the view |
| `bus` | `on\|off` | `off` | c4rizer extension: a line may join another line into the same box, ending in a dot (CORE-8); lanes (CORE-12) are always on |
| `labelWidth` | `N` | `24` | c4rizer extension: characters per label line |
| `maxCrossings` | `N` | `0` | c4rizer extension: crossing budget of the view |
| `focus` | `<id>...` | none (all lines) | c4rizer extension: draw only lines touching these boxes |
| `hide` | `<src> -> <dst>` | none | c4rizer extension: leave this line out; one per `hide` line |
| `at` | `<id> <x> <y>` | auto-placed | c4rizer extension: box centre in px, multiples of 10 |
| `size` | `<id> <w> <h>` | per kind (CORE-2) | c4rizer extension: box size in px, multiples of 20 so half-sizes stay on the grid |
| `styles` | `{ }` inside `views` | — | holds `element` styles |
| `element` | `"<tag>" { }` in `styles` | — | style for every element with that tag |
| `relationship` | `"<tag>" { ... }` in `styles` | — | accepted and kept byte for byte, no effect until P2-12 (so the prototype example opens, DSL-14) |
| `background` | `#rrggbb` | `#dddddd` | box fill |
| `color` | `#rrggbb` | `#000000` | box text colour |
| `shape` | `Person\|Cylinder\|Box` | `Person` for people, else `Box` | box shape |

The prototype's `tries` and `seed` options are dropped: routing is
deterministic (CORE-25).

#### Requirements

Model

- **DSL-1** Parsing yields every `person`, `softwareSystem`,
  `container` and `component` with its id, name, description,
  technology, tags and parent, whether the values are positional or
  set by `description`, `technology` and `tags` lines in its block.
- **DSL-2** Parsing yields every relationship with source, destination,
  description, technology and tags, whether declared at top level or
  nested, with `this` or a left-out source meaning the enclosing
  element.
- **DSL-3** Parsing yields `element` styles, written in a `styles`
  block inside `views` as in Structurizr, with `background`, `color`
  and `shape` (`Person`, `Cylinder`, `Box` by default).
- **DSL-4** In phase 1, any construct outside the keyword table above
  is a parse error naming the line, the column and the construct, and
  saying when it is planned if it is in a later phase; nothing is
  skipped silently.
- **DSL-5** A relationship or view entry naming an undeclared id is an
  error naming the id.
- **DSL-15** Every element has the implicit tags `Element` and its kind
  name (`Person`, `Software System`, `Container`, `Component`) before
  its declared tags, and every relationship has `Relationship`; style
  is resolved per property across the element's tags in tag order,
  later tags overriding, and a property no tag sets takes the default
  in the keyword table (shape: `Person` for a person, `Box` otherwise).
  The tag order is `Element`, then the kind tag, then the declared tags
  in the order written; each property takes the value of the last tag
  in that order that sets it, and when the same tag has two style
  blocks, the later block wins.
- **DSL-16** Every span the parser records carries a source-file id
  (in phase 1 always the root file), so that `!include` (P2-5) needs no
  change to the span type.

Views and positions (replaces the prototype's `layout.json`)

- **DSL-6** A view declares kind (`systemContext`, `container`,
  `component`), scope, optional key and optional description, as in
  Structurizr; the title drawn is the `title` line, else the
  description; the body accepts every view keyword in the table above,
  including `autoLayout`, kept byte for byte.
- **DSL-7** A view's box positions are `at <id> <x> <y>` lines (box
  centre, multiples of 10) and its size overrides are
  `size <id> <w> <h>` lines (multiples of 20), at most one of each per
  box; `<id>` may be a dotted path `a.b.c`, naming the element keyed
  `c` whose parent is keyed `b` inside `a`. In phase 1 every element
  and no relationship has an identifier: an element without `<id> =`
  and a named relationship `r = a -> b` are DSL-4 errors naming
  phase 2. A view with no key gets `<Kind>-NNN` (`SystemContext-001`,
  numbered per kind in declaration order), as Structurizr does.
  A `focus` or `hide` id with no box of its own in the view is lifted
  to its nearest drawn ancestor; when it has none it is ignored.
- **DSL-8** An element included in a view but without an `at` line is
  placed automatically (CORE-22); positions are written back only as
  EDIT-26 says.

Writing back

- **DSL-9** Moving one box rewrites only that box's `at` line, or adds
  one after the view's last `at` line (before the view's closing brace
  when it has none), copying the indentation and line ending of the
  line above; when the line above is the view head, the new line gets
  the view's body indentation (the first body line's indentation, else
  the view's indentation plus one level of the file's indent unit, four
  spaces by default; a one-line view `{ ... }` is first split onto
  lines); a new `size` line is placed the same way after the last
  `size` line, else after the last `at` line; every other byte of the file is unchanged, except on the
  first write in a view (EDIT-26).
- **DSL-10** Editing the name, description or technology of an
  element, the description or technology of a relationship, or the
  title or subtitle of a view rewrites only that string literal, with
  `"` and `\` escaped; a newline is refused; when the literal is absent
  the edit inserts it, with `""` placeholders for skipped positional
  arguments; a missing view `title` or `subtitle` line is inserted as
  the first line of the view body, indented per DSL-9; every other byte
  is unchanged.
- **DSL-13** Changing an element's colour, text colour or shape writes
  to the `styles` block for the target the user picked: `all <tag>`
  edits or adds the property in that tag's `element` block; `this box
  only` adds a tag named after the element's full dotted id to the
  element, suffixed `-2`, `-3`... when another element already carries
  that tag (a `tags` line as the last line of its block, creating the
  block if needed) and an
  `element "<id>"` block at the end of `styles`; a missing `styles`
  block is created as the last block of `views` (styles live inside
  `views`, as in Structurizr), and a missing `views` block as the last
  block of the workspace body, both indented per DSL-9; those are the only
  bytes changed, and "reset to tag style" removes that tag and block
  (and a `styles` or `views` block only when it holds nothing but what
  `edit` wrote).
- **DSL-11** Parse, then write back with no edits, gives a file
  byte-identical to the input, comments and blank lines included, for
  LF and CRLF files, with or without a final newline.

Migration and fixtures

- **DSL-12** `example/workspace.dsl` + `example/layout.json` from the
  prototype are converted once by hand into `tests/data/example.dsl`, a
  c4rizer file that renders the same four views (keys as in the prototype:
  `C1`, `C2`, `C3-serving`, `C3-startup`); it is checked in and is the golden-test input.
- **DSL-14** `docs/workspace.dsl` as checked in and
  `tests/data/example/workspace.dsl` (a checked-in copy of the
  prototype's `example/workspace.dsl`) parse with no error and every
  view in them renders; this is an oracle test.

#### Open holes

- Constructs of later phases are rejected per DSL-4 during phase 1;
  from phase 2 they are parsed (P2-1, P2-2).

### 4.2 Core

#### Responsibility

Parse, place, route, place labels, lint, build the scene, export it
to SVG and PNG. No UI, no file watching, no windowing; every function
is callable from a test.

#### Requirements

Views and rendering

- **CORE-1** Each view builds one scene holding a title, a subtitle,
  a generated key covering every shape, colour and line style drawn,
  and a list of nested dashed boundaries (phase 1 fills at most one:
  the scope's children plus 40px, with the scope's name tag at its
  bottom left). The canvas and the exporters draw only what the scene
  holds. The name tag reads the scope's name followed by its type in
  brackets (`[Software System]` on a container view, `[Container]` on
  a component view), in bold. The title (18px Inter Bold) and subtitle
  (12px Inter Regular) sit at the bottom left below the drawing, the
  key below them. Each key entry is one distinct box look (shape,
  fill, text colour) or line look (colour, end), labelled with the
  most specific tag of the boxes drawn that way (several tags joined
  by commas); line entries read `Relationship`.
- **CORE-2** People draw as the person shape and `Cylinder` elements as
  cylinders; every box shows name, `[type: technology]` (type
  `External Software System` for a software system tagged `External`)
  and a wrapped description; a box with no `size` line is 200x180
  (person), 240x140 (software system, container) or 200x120
  (component). The box name is 14px Inter Bold, the type line and
  description 11px Inter Regular on a 14px pitch, with 10px padding;
  lines are 2px wide, arrowheads 10px, chevrons 6px, and join dots
  have a 4px radius.
- **CORE-3** Every line takes the colour of the box it points to,
  unless a relationship style sets its colour (P2-12).
- **CORE-4** With `technology off`, no technology appears on any line
  of that view.
- **CORE-5** A relationship between two elements not both in the view
  is lifted to their nearest drawn ancestors; a relationship declared
  between the two drawn elements wins over a lifted one, otherwise the
  first declared wins; a relationship whose ends lift to the same box
  is not drawn; duplicate lifted lines are drawn once. A lifted line
  between an element's box and the box of one of its own ancestors is
  dropped, in either direction (child to ancestor, ancestor to child),
  as Structurizr creates no implied relationship between parent and
  child; this applies only to lifted lines: a relationship the author
  wrote directly between a parent and its child is still drawn.
- **CORE-30** Text is wrapped in core with a fixed per-character width
  table derived from one bundled font (no system font metrics), and the
  scene carries the final lines; the editor canvas and the PNG
  rasterizer use that font file, the SVG names it first in
  `font-family` with a `sans-serif` fallback, every text of a scene of
  `example/` fits its box or label rectangle, and text that cannot fit
  is lint warning L25. The bundled font is Inter (SIL OFL 1.1, owner's
  choice 2026-11-15).

Router (`SKILL.md` "Lines", `c4.py` `route`)

- **CORE-6** Every line segment is at 0°, 45° or 90°, every vertex
  sits on the 10px grid, every bend turns by 45°, and every segment is
  at least 20px long.
- **CORE-7** No line passes through a box other than its own ends;
  lines stay inside the scene and at least 10px from any box other
  than at their own ports.
- **CORE-8** No two lines share a segment, except with `bus on`: a
  line may join a line into the same box only after 90px of its own
  run and at 90° or less to the trunk's direction; a joined line ends
  in a dot, not an arrowhead; a join is not a crossing; a crossing goes
  straight through, never at a bend.
- **CORE-9** Each line has its own port on each box, so no two
  arrowheads touch the same point; a line meets its box square to the
  side, at least 20px from a corner; a person takes at most one line on
  top, at its centre, and side lines only below its head (at least 80px
  from its top); a cylinder's top and bottom ports sit within 30px of
  its centre line.
- **CORE-10** Lines are solid and end in one arrowhead (or a join
  dot); every segment of 100px or more carries a direction chevron at
  its middle, or three at 1/4, 1/2 and 3/4 when it is 320px or more,
  except a chevron that would fall within 8px of a label.
- **CORE-11** On `example/`, every view routes with crossings no higher
  than the prototype's (C1 0, C2 0, C3 1, C3-startup 0).
- **CORE-12** Lines heading the same way run as lanes exactly 20px
  apart; on `example/` the length of line running 20px beside another
  (horizontal and vertical runs heading the same way, summed once per
  pair of lines) is at least the prototype's: C1 0px, C2 70px, C3 220px,
  C3-startup 110px (measured once on the prototype output, 2026-11-15).
- **CORE-25** Routing is deterministic: the same file gives the same
  scene, and the same SVG bytes, on every run and machine; scene
  coordinates are integers or fixed-precision decimals; tested by
  building each `example/` view twice and comparing SVG bytes.
- **CORE-13** When a line cannot be routed, routing does not stop: the
  router retries (other edge orders or rip-up); a line still unroutable
  is reported by id (L1), never dropped silently. An unroutable line is
  drawn straight from its source box's edge to its destination box's
  edge, on the line between their centres, with an arrowhead and no
  label, outside the CORE-6..9 checks; L1 names it.
- **CORE-32** Routing has an iteration cap and always returns a scene,
  with any line it could not route reported per CORE-13.

Labels

- **CORE-14** Each label (description wrapped at `labelWidth`
  characters, then `[technology]` on its own line) sits on its own
  line, at least 30px from the line's start and 50px from its
  arrowhead, and overlaps no box, no other label, no other line, the
  title, the key or the boundary name tag, except as CORE-15 allows; a line joining a trunk whose
  label has the same description and technology has no label of its
  own. Label text is 11px Inter Regular on a 13px line pitch, padded
  5px left and right and 3px top and bottom; the 30px and 50px gaps are
  measured along the line to the label's centre.
- **CORE-15** A label that can only be placed over another line is
  drawn and reported as warning L23; a label with no clear spot is
  drawn at the middle of its line and reported as warning L24; both
  name the relationship.

Lint (exit-code rules from `SKILL.md` and `c4.py` `lint`)

- **CORE-16** Lint reports, per view, each rule of the lint table
  below that is broken, with the ids involved, at the level the table
  gives; errors fail the run (CLI-2), warnings do not. Each finding's
  text is the prototype `c4.py` `lint` message for that rule, naming
  elements by quoted name; L4 ends with the CORE-26 split hint
  `split the view (focus, hide); boxes involved: "<name>", ...`; in
  L22, `<src>`, `<dst>` and `<scope>` are element names.
- **CORE-17** Crossings above the view's `maxCrossings` (default 0)
  are an error (L4).
- **CORE-18** A box whose centre or half-size is off the 10px grid, or
  two boxes whose edges are less than 60px apart, is an error (L5, L6).
- **CORE-19** C1 flags boxes that are not people or software
  systems, technology on lines, and protocol words in box names and
  labels; C2 rejects components; C3 must show at least one component of
  its scope (L15 to L21; each rule's level is in the lint table).
- **CORE-20** Each box needs name and description; containers and
  components also need technology; each relationship needs an intent
  label, and descriptions stay short (L9 to L13; levels per the lint
  table).
- **CORE-21** For each container with at least one component view,
  every relationship with one of its components at either end is drawn
  on at least one of those views, otherwise an error names it (L22).
  L22's `<scope>` is the source's container if the relationship fails
  there, else the destination's.

Lint table. "Box" and "line" mean what the view draws, after lifting.

| id | level | exact condition |
|---|---|---|
| L1 | error | a line could not be routed |
| L2 | error | a segment is not at 0°, 45° or 90° |
| L3 | error | a segment passes through the inside of a box other than its own two ends |
| L4 | error | crossings > `maxCrossings`; a crossing is two segments of different lines that cross or touch, except where a joined line ends on the trunk it joins (same destination); the message also gives the CORE-26 split hint |
| L5 | error | a box centre, half-width or half-height is not a multiple of 10 |
| L6 | error | two boxes, each grown by 30px on every side, overlap (edges less than 60px apart) |
| L7 | warning | more than 3 boxes, and distinct centre rows + distinct centre columns > boxes + 1 |
| L8 | error | the view has no title (no `title` line and no description) |
| L9 | error | a box has an empty name or an empty description |
| L10 | error | a container or component has no technology |
| L11 | warning | a box description is over 110 characters |
| L12 | error | a line has no label |
| L13 | error | a label, trimmed and lowercased, is one of `uses`, `use`, `calls`, `sends`, `reads`, `writes` |
| L14 | warning | both A -> B and B -> A are drawn (reported once per pair) |
| L15 | error | a C1 line shows a technology |
| L16 | error | a C1 box is not a person or a software system |
| L17 | warning | a C1 view has more than 8 boxes |
| L18 | error | a C1 box name or line label contains, as a whole word and case-sensitive, one of `SMTP`, `HTTP`, `HTTPS`, `SQL`, `JSON`, `REST`, `gRPC`, `TCP`, `API`, `VAPID`; a word is a maximal run of ASCII letters and digits |
| L19 | error | a C2 view shows a component |
| L20 | warning | a C2 line has a container at one end, no person at either end, and no technology |
| L21 | error | a C3 view shows no component whose parent is the scope |
| L22 | error | CORE-21 coverage fails; reported once per relationship, outside any view |
| L23 | warning | a label covers another line |
| L24 | warning | a label has no room |
| L25 | warning | a text does not fit its box or label rectangle (CORE-30) |
| L26 | error | a box that is not a child of the scope overlaps the boundary rectangle |

Placement

- **CORE-22** Auto-placement puts every unplaced box of a view on the
  grid with no overlap, without moving any box that already has a
  position.
- **CORE-23** With every `at` line removed from `tests/data/example.dsl`,
  auto-place then route gives, per view: no unroutable line, no label
  without room, crossings within the per-view cap (C1 0, C2 0,
  C3-serving 1, C3-startup 0: the prototype's routed crossings on its
  hand layout, `c4.py` on `example/`, measured 2026-11-15), people on
  the top row, boxes outside the scope outside the boundary, column
  pitch at least 340px and row pitch at least 280px.
- **CORE-26** When a view cannot meet its crossing budget after
  placement and routing, lint says so and suggests splitting the view
  (`focus`, `hide`), naming the boxes involved in the crossings.
- **CORE-29** Adding an unplaced box to a view never changes the
  position of a box that was already drawn in the previous scene of
  the same session. A kept position wins over CORE-22's no-overlap
  rule; lint reports the overlap (L6).

Speed (checked by the release perf oracle, 5.2)

- **CORE-24** Re-rendering view `c3` of `example/` (the heaviest) after
  a single box move, full route, takes under 100 ms in a release build.
  `TODO(srs)`: confirm the budget after the first release measurement;
  the prototype takes about 5 s a view.
- **CORE-27** Startup path: from file text to the routed scene of view
  `c3` of `example/` takes under 300 ms in a release build.
  `TODO(srs)`: confirm 300 ms after the first release measurement.
- **CORE-28** Routing one view needs no other view: a caller can ask
  for one view's scene without the others being routed.
- **CORE-31** A generated view of 40 boxes and 60 lines (fixed seed,
  checked in as test data) routes in under 1000 ms in a release
  build.

#### Interfaces (intent, not signatures)

Diagram file in, per view: a scene (boxes, routed lines, labels,
title, key, boundaries) and a lint report (errors, warnings). Each
scene item has a view-local key that maps to a model id (in phase 1 the
key is the model id; deployment and dynamic views draw one element or
relationship several times); `at`/`size` lines and edits address the
key, so the editor can hit-test and edit it. Scene out: SVG text, PNG
bytes. Incremental re-route during a drag is an internal matter as
long as CORE-24 and EDIT-25 hold.

#### Open holes

CORE-24, CORE-27, CORE-31 numbers (measured when pipeline is built).

### 4.3 CLI

#### Responsibility

Render and lint from a shell, for scripts and agents.

#### Requirements

- **CLI-1** `c4rizer render FILE -o DIR` writes one SVG per view,
  named `<view key>.svg`, and prints per view, in order:
  `== <key>: <title>`, then indented by three spaces
  `<n> boxes, <m> lines, <c> crossings (budget <b>)`,
  `alignment: <r> rows, <k> columns for <n> boxes`, one
  `warning: <text>` per warning, one `ERROR: <text>` per error and
  `wrote <path>`; coverage errors (L22) follow all views as
  `ERROR: <src> -> <dst> is in the model but on no component view of "<scope>"`.
- **CLI-2** Exit code is 0 when no view has a lint error, 1 when any
  does, 2 on a parse or I/O error.
- **CLI-3** `c4rizer lint FILE` does the same checks and prints the
  same report without writing files.
- **CLI-4** `--png` also writes `<view key>.png` per view, at twice the
  SVG width unless `--width N` gives the width in px.
- **CLI-5** The golden test on `example/` has two checks: (1) against
  the prototype SVGs: same boxes, same lines, same joined lines, same
  labels present, crossings no higher (CORE-11); (2) against
  c4rizer's own four SVGs, approved once by the owner and checked in:
  byte-identical; a deliberate change regenerates them and shows the
  diff in the PR.

### 4.4 Editor

#### Responsibility

A native desktop window that draws the scene on an interactive
canvas, follows the diagram file live, and lets the architect arrange
boxes and edit text in place. The editor is a headless state machine
(input events, file events and worker results in; file writes, worker
requests and a draw list out) plus a thin eframe shell; every EDIT
requirement is tested against the state machine with a temp-dir file,
without opening a window. Stack: eframe/egui (chosen by the agent on
the owner's delegation, 2026-11-15); `notify` for file watching and
`rfd` for the Open and Save dialogs (approved by the owner,
2026-11-15); PNG through the core exporter (`resvg`, approved by the
owner, 2026-11-15); the bundled font of CORE-30.

#### Requirements

- **EDIT-1** Saving the diagram file from another program updates
  the open view without a restart or a manual reload, keeping the
  selected view, zoom and pan.
- **EDIT-2** Dragging a box snaps it to the 10px grid and re-routes the
  lines while dragging.
- **EDIT-3** Releasing a dragged box writes its position to the diagram
  file (DSL-9, EDIT-26).
- **EDIT-4** Auto-place never locks a box: any box placed automatically
  can be dragged afterwards and keeps the dragged position.
- **EDIT-5** With a box, a line or the view title selected, Enter or F2
  edits its first field in place (name; line description; title) and
  Tab moves to the next field (element description, technology; line
  technology; subtitle); confirming writes it to the diagram file
  (DSL-10), Escape cancels; editing a lifted line edits the
  relationship it was lifted from, and the details panel names that
  relationship and lists any others merged into the line. Gestures per
  EDIT-28.
- **EDIT-6** Lint findings are shown next to the view and update with
  each render.
- **EDIT-7** "Export view" and "Export all views" write SVG or PNG
  through a save dialog that defaults to the diagram file's folder and
  the name `<view key>.svg` or `<view key>.png`, PNG at twice the SVG
  width by default. "Export all views" asks for a folder, defaulting
  to the diagram file's folder, and writes every declared view there
  as `<view key>.svg` or `<view key>.png`, replacing files of the same
  name.
- **EDIT-8** All views of the file are listed; picking one shows it.
- **EDIT-9** The canvas pans and zooms with the gestures of EDIT-28;
  zoom does not change the diagram, only the view of it.
- **EDIT-10** Selecting a box or line (EDIT-28) shows its model details
  (name, type, description, technology, tags) and the style panel.
- **EDIT-11** A parse error in the file leaves the last good diagram on
  screen and shows the error with its line and column; while the file
  on disk does not parse, the canvas is read-only (no move, resize,
  text or style edit, no undo or redo) and editing resumes on the next
  good parse; opening a file that does not parse shows the error and an
  empty canvas, and still hot-reloads.
- **EDIT-12** The canvas draws from the scene only; nothing on the
  canvas is drawn that the SVG export of the same view would not show,
  apart from selection and hover marks.

Opening and starting

- **EDIT-13** The app starts with no argument and offers Open (file
  dialog) and a list of recently opened files; `c4rizer-editor FILE`
  opens a file directly; per-user UI state (recent files, last view,
  zoom and pan per file) lives in `$XDG_CONFIG_HOME/c4rizer/`, never
  holds diagram content, and on open the last viewed view is shown if
  it still exists, else the first declared.
- **EDIT-14** `c4rizer-editor [FILE] --startup-report` prints the ms
  from process start to the first presented frame and exits; the budget
  is 500 ms with `example/` and 300 ms with no file, checked by a QA
  script and by one smoke test that launches the binary; other views
  route in the background and never delay the first one (CORE-28).
  `TODO(srs)`: confirm both budgets once the window backend is
  measured.

C4 levels

- **EDIT-15** A box is enterable when its element has at least one
  child in the model (opens the default view when no view is scoped to
  it), or a container view scoped to it (for a system) or a component
  view scoped to it (for a container); a left click on an
  enterable box opens its view (container view of a system, component
  view of a container), and on any other box selects it; enterable
  boxes show a hover mark; when several views are scoped to it, the
  first declared opens and the view list (EDIT-8) shows the others;
  when none exists, a default view (`include *`, every box auto-placed)
  opens, and it is written to the file on the first write made in it
  (move, resize, style, or a title or subtitle edit), appended as the last view in `views` with key
  `<scope id>-<kind>` (`<kind>` is the view keyword: `container`,
  `component`, `systemContext`); `edit` writes it, as the first patch of that
  write. In a file with no `views` block (model only, valid Structurizr)
  every view shown is a default view (a system context default view of
  a software system included, key `<system id>-systemContext`), and on
  the first write in one `edit` first creates the `views` block as the last block of the
  workspace body, indented per DSL-9; undoing that write removes the
  `views` block only when it holds nothing but what `edit` wrote.
  A default view has no title line; its drawn title is the scope's
  name. Gestures per EDIT-28.
- **EDIT-16** The breadcrumb is derived from the open view's scope in
  the model (workspace / system / container), not from click history;
  clicking a crumb opens the first declared view scoped to that
  element; Alt+Left or Backspace opens the parent crumb, and Backspace
  is ignored while a text field has focus. The workspace crumb, and
  Alt+Left or Backspace on a system context view, open the first
  declared system context view, else the first declared view.

Editing

- **EDIT-17** Selected boxes can change colour, text colour and shape
  from a side panel, with the target choice of DSL-13; the change is
  written per DSL-13, drawn at once, and is one undo step for all
  selected boxes.
- **EDIT-18** A box can be resized on the grid by its handles, never
  below the size that fits its wrapped text (CORE-30); a resize
  rewrites or adds only that box's `size` line (DSL-7), except on the
  first write in a view (EDIT-26).
- **EDIT-19** Undo and redo cover every change the app wrote to the
  file (moves, resizes, text, styles), at least 100 steps, for the
  session; steps are semantic operations with their previous value, not
  byte patches, survive external reloads and are applied through
  EDIT-24; a step whose target no longer exists is skipped and the
  status bar says so; a multi-box operation is one step.
- **EDIT-20** Several boxes selected with Shift-click or a rubber band
  (EDIT-28) move together, and can be aligned (left, centre, right,
  top, middle, bottom) and distributed evenly.
- **EDIT-21** The app never creates or deletes elements or
  relationships; it changes text, layout and style only (section 2).
- **EDIT-26** The first write in a view (move or resize) writes an `at`
  line for every box of that view at its current position, as one undo
  step; after that a move changes only the moved box's line (DSL-9).
- **EDIT-28** Input gestures: a left click (pointer travel under 4
  screen px) enters an enterable box, else selects; left drag on a box
  moves it; left drag on empty canvas draws a rubber band; middle drag
  or Space+drag pans; the wheel zooms at the cursor; Ctrl-click or
  right-click selects without entering; Shift-click toggles selection
  and never enters; a click on empty canvas or Escape deselects; Enter
  or F2 edits (EDIT-5); Ctrl+Z (Cmd+Z on macOS) undoes; Ctrl+Shift+Z
  and Ctrl+Y redo (EDIT-19).

Working beside a text editor

- **EDIT-22** Every write is atomic (write to a temporary file, then
  rename), keeps the file's permissions, writes through a symlink to
  its target, and the app does not reload on its own writes.
- **EDIT-23** The watcher sees saves done by rename (it watches the
  file's directory), so editors that save that way still trigger
  EDIT-1.
- **EDIT-24** Before writing, the app checks the file on disk is the
  one it last read. If it changed, the app reloads first and re-applies
  the user's action to the new text (move box X, set label of Y); if
  the target no longer exists, the action is dropped and the status bar
  says so. A text editor with unsaved changes is not visible to the app;
  the editor's own changed-on-disk prompt handles that side.
- **EDIT-25** Routing never runs on the UI thread: while a box is
  dragged, only the lines touching it are re-routed, on a worker that
  cancels stale requests, and the box moves at once while its lines
  follow when routing finishes; no UI update waits on the worker
  (test: 100 drag events with a stalled worker); after release the
  full view is re-routed and the scene shown equals the scene the core
  builds from the saved file with the session's previous positions
  (CORE-29). With EDIT-26, every box has an `at` line after the first
  write, so from then on the editor and the CLI give the same scene
  from the file alone.
- **EDIT-27** If the diagram file disappears, the last diagram stays
  read-only until it reappears; a failed write reverts the canvas to
  the file's state and shows the I/O error; if the open view no longer
  exists after a reload, the first declared view opens.
- **EDIT-29** On `example/`, a save from another program shows the
  updated view in under 200 ms in a release build, measured from the
  file event to the scene handed to the canvas.

### 4.5 Diagram exports

- **EXP-1** Exported SVG has no script and no external reference
  (tested by scanning the output) and names the bundled font first
  (CORE-30). The SVG does not embed the font: it names it in
  `font-family` with a `sans-serif` fallback and carries no
  `@font-face`.
- **EXP-2** PNG export of a view is its SVG rasterized at the requested
  width with the bundled font only (no system fonts).

### 4.6 Phase 2

Stubs, grouped by ROADMAP phase 2 row (a row may own several); each
row's stubs become one testable sentence each at the row's step 2. The parity table (section 2) lists every
feature these stubs cover.

dsl-breadth

- **P2-1** The parser accepts the complete Structurizr DSL grammar,
  deployment and dynamic constructs included, and every file of the
  compat corpus (P2-14) parses with zero errors.
- **P2-2** A view whose type is not yet drawable appears in the view
  list marked "not yet supported" instead of failing the file.
- **P2-3** A construct that is accepted but has no effect (`!script`,
  `!plugin`, `configuration`, and others the parity table marks so)
  shows a visible notice naming it, in the CLI report and the editor.
- **P2-4** `group` blocks draw as nested dashed boundaries (CORE-1).
- **P2-5** `!include <path>` reads a local file; a text edit goes to
  the file the literal came from, `at` and `size` lines always go to
  the root file, and the editor watches every included file.
  A URL must be HTTPS (parity table). `TODO(srs)`: fetch, cache and
  offline behaviour for URL includes.
- **P2-6** With `!identifiers hierarchical`, ids are dotted paths and
  are accepted as such in `at`, `size`, `focus`, `hide` and edits.
- **P2-7** `!impliedRelationships`, archetypes, `!docs`, `!adrs` and
  `terminology` each parse and take the effect the Structurizr
  reference gives them (one test each). `TODO(srs)`: `!const`/`!var`
  and `branding` have no parity row (not found in the reference pages
  read); confirm against the reference, then add rows or drop them.
- **P2-16** `workspace extends <file|url>` loads the base workspace
  and applies the file on top of it. `TODO(srs)`: URL fetch (same
  rules as P2-5).
- **P2-17** `!element`, `!elements`, `!relationship`,
  `!relationships` (and the deprecated `!extend`, `!ref`) and custom
  `element` declarations build and modify the model as in Structurizr.
- **P2-18** `tag`, `url`, `properties`, `perspectives`, workspace
  `name`/`description`, `configuration scope` (a validation rule) and
  the `structurizr.*` view-set properties of the parity table are
  parsed into the model and act as the table notes.
  `TODO(srs)`: one requirement per item.

landscape-filtered

- **P2-8** A `systemLandscape ["key"] ["description"]` view with
  `include *` draws every person and software system of the model,
  with no boundary, under the same router and lint rules as C1.
  `TODO(srs)`: which lint rules apply to a landscape view.
- **P2-9** A `filtered <baseKey> include|exclude "<tags>" ["key"]
  ["description"]` view draws its base view keeping, or dropping, the
  elements and relationships with those tags, and takes the base
  view's `at` lines. `TODO(srs)`: whether a filtered view may carry its
  own `at` lines.
- **P2-19** A `custom` view draws custom elements, and an `image` view
  with a local file source shows that PNG or SVG. `TODO(srs)`: image
  views with a URL source.
- **P2-20** View statements beyond phase 1 work as in Structurizr:
  `include *?`, element and relationship expressions with `&&`/`||`,
  relationship patterns in `exclude`, `default` and view `properties`.
  `TODO(srs)`: one requirement per expression form.

styles-themes

- **P2-10** Every Structurizr element style property is parsed and
  drawn in canvas, SVG and PNG alike. `TODO(srs)`: list from the parity
  table (stroke, strokeWidth, border, opacity, fontSize, width, height,
  icon, metadata, description, and others).
- **P2-11** Every Structurizr shape draws in canvas, SVG and PNG alike,
  with ports that keep CORE-9. `TODO(srs)`: list from the parity table
  and port rules per shape.
- **P2-12** `relationship "<tag>"` styles set line colour, thickness
  and dash, and a set colour overrides CORE-3. `TODO(srs)`: what
  Structurizr `routing` and `position` mean under the circuit-board
  router.
- **P2-13** `theme` and `themes` apply the theme's styles below the
  file's own styles. `TODO(srs)`: themes are URLs; decide bundled
  copies, a local cache or a fetch.
- **P2-21** `styles { light {...} dark {...} }` give each view a light
  and a dark rendering; the editor switches mode and SVG/PNG export
  takes a dark mode option (Structurizr `-mode dark`).

compat-fixtures

- **P2-14** Every `.dsl` in `tests/data/structurizr/` (the official
  Structurizr examples plus `docs/workspace.dsl`) opens with zero
  parse errors and every drawable view renders; this is an oracle
  test. `TODO(srs)`: pick the example set and its licence.
- **P2-15** `c4rizer import-layout workspace.json FILE.dsl` writes, for
  each view key present in both, an `at` line for every element
  position stored in the Structurizr `workspace.json`, snapped to the
  10px grid, and changes nothing else in `FILE.dsl`. `TODO(srs)`:
  coordinate mapping (Structurizr stores top-left corners; `at` is the
  centre).

cli-breadth (ROADMAP row `cli-breadth`, phase 2)

- **P2-22** The CLI adds `validate`, `inspect` (exit code = violation
  count), `list` (lists elements), `version` and `help`, with the
  meaning of the Structurizr commands of the same name.
  `TODO(srs)`: argument names and output format.
- **P2-25** SVG export (CLI `render` and the editor's export) offers an
  option to embed the font or outline the text, instead of naming it
  (EXP-1). Not built in phase 1.

editor-breadth (ROADMAP row `editor-breadth`, phase 2)

- **P2-23** Canvas commands of the Structurizr diagram editor: run
  auto-layout again on demand, bring all boxes back onto the canvas,
  canvas size decrease/auto/increase, centre diagram or selection,
  fit to width/height/content, fullscreen, keyboard shortcuts for
  align and distribute, move a label with Up/Down (writes `position`),
  cycle routing with `r` and toggle jumps with `j`.
- **P2-24** Viewing aids: keyboard previous/next/back navigation
  between views, open an element or relationship `url`, toggle
  descriptions and metadata, tooltips, filter by tags and a
  perspectives view. `TODO(srs)`: one requirement per item.

### 4.7 Phase 3

Stubs, grouped by ROADMAP phase 3 row; each becomes one testable
sentence at the row's step 2.

deployment-view

- **P3-1** `deploymentEnvironment`, `deploymentNode`,
  `infrastructureNode`, `softwareSystemInstance` and
  `containerInstance` build a deployment model with parents.
- **P3-2** A `deployment <system|*> <environment> ["key"]
  ["description"]` view draws deployment nodes as nested boundaries
  with their instances inside, each drawn instance under its own
  view-local key. `TODO(srs)`: lint rules and how the router treats
  nested node boundaries.

dynamic-view

- **P3-3** A `dynamic <scope|*> ["key"] ["description"]` view draws its
  listed relationships in order, each label prefixed with its sequence
  number, and the same relationship listed twice is two lines with two
  view-local keys. `TODO(srs)`: parallel sequences and `{ }` blocks.
- **P3-7** An `animation` block steps through a view's boxes in order;
  the editor steps forward/back and starts/stops it, and PNG/SVG export
  can write one image per step. `TODO(srs)`: whether animation of
  static views belongs here or in its own row.

vertices

- **P3-4** A line can carry hand-added vertices, written to the file
  as one line per relationship in the view; the line passes through
  them, still at 0°, 45° or 90° and on the grid, and the router routes
  only the parts between them. `TODO(srs)`: the syntax (proposal:
  `via <src> -> <dst> <x> <y> ...`).
- **P3-5** Removing a line's vertices returns it to the router; with no
  vertices anywhere, the scene equals the phase 1 scene (the router
  stays the default).

exporters

- **P3-6** Each export format the parity table gives phase 3 is written
  by the CLI (`export`) and by the editor's export, with a golden test
  on `example/`: `plantuml`, `plantuml/c4plantuml`, `mermaid`,
  `websequencediagrams`, `static`, `json` (with the `structurizr.dsl`
  properties), `theme` and DOT.
- **P3-8** `merge` applies the layout of a Structurizr workspace JSON
  to a workspace, and `generate system-landscape` merges several
  workspaces into one landscape. `TODO(srs)`: argument names.

explore (ROADMAP row `explore`, phase 3)

- **P3-9** The editor offers graph exploration and a viewer for the
  documentation and decisions attached by `!docs` and `!adrs` (P2-7).

## 5. Cross-cutting

### 5.1 Errors

Parse and I/O errors carry file, line and column where they
exist, and a sentence saying what to change. Lint findings are data,
not errors.

### 5.2 Performance

Startup first (the owner's main complaint about Structurizr):
CORE-27, CORE-28, EDIT-14. Then re-render after a change: CORE-24,
CORE-31, CORE-32, EDIT-25, EDIT-29. Routing is the hot path. The
timed view is `c3` of `example/`, the heaviest. Timing tests are named
`perf_*` and `#[ignore]`d in debug; `QA.md` already has the release
perf oracle item `cargo test --workspace --release -- --ignored perf_`,
so every core budget is checked on every oracle run; EDIT-14 is
checked by its `--startup-report` smoke test. A disk cache of
routed scenes is not a requirement: it is added only if CORE-27 cannot
be met otherwise, since it would add a second file beside the single
diagram file.

### 5.3 Safety and `unsafe`

No `unsafe` expected.

### 5.4 Test data

No personal names in examples or test data; people are roles.
`tests/data/` holds the prototype's `example/` (model, layout, four
reference SVGs) under `tests/data/example/`, its hand conversion
`tests/data/example.dsl` (DSL-12), c4rizer's approved SVGs (CLI-5) and,
in phase 2, `tests/data/structurizr/` (P2-14).

## 6. Glossary

- **Bus**: c4rizer's circuit-board line style: lanes (lines heading the
  same way, side by side 20px apart, CORE-12) plus joins (a line
  merging into another that goes to the same box, ending in a dot,
  CORE-8; switched by the `bus` option).
- **Trunk, join**: with `bus on`, a line may end on another line into
  the same box (the trunk); the joint is drawn as a dot.
- **Port**: the point on a box edge where one line starts or ends.
- **Lifting**: drawing a relationship between two hidden elements as a
  line between their visible ancestors.
- **Enterable**: a box whose element has a child in the model, or a
  container view (system) or component view (container) scoped to it
  (EDIT-15).
- **View-local key**: the id of one drawn item in one view; maps to a
  model id.
- **Golden test**: rendering `example/` and comparing with the
  reference SVGs (CLI-5).
