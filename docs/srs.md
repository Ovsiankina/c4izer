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
| DSL keyword | `-/>` remove relationship (deployment environment) | 3 | Deployment; differs (accepted): Structurizr 6.2.4 refuses it, c4rizer accepts it as a superset (P3-108) |
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
| export format | `static` (HTML site) | 3 | differs (accepted): c4rizer's own script-free HTML+SVG site, not Structurizr's JS viewer (P3-423) |
| export format | `json` (Structurizr workspace JSON) | 3 | |
| export format | `theme` (JSON theme from workspace styles) | 3 | |
| export format | Custom exporter by FQCN (`plugins` JAR) | never | Needs the JVM |
| export format | DOT (Graphviz) | 3 | Not in Structurizr CLI 6.2.4; in structurizr-export 6.2.4 (`DOTExporter`) |
| export format | `ilograph` | 3 | Not in Structurizr CLI 6.2.4; in structurizr-export 6.2.4 (`IlographExporter`) |
| CLI command | `c4rizer render` | 1 | CLI-1; covers `export -format svg/png` |
| CLI command | `c4rizer lint` | 1 | CLI-3 |
| CLI command | `export` (non-SVG/PNG formats) | 3 | |
| CLI command | `validate -workspace` | 2 | |
| CLI command | `inspect -workspace [-severity]` | 2 | Exit code = violation count; `-inspector` Java class: never |
| CLI command | `list -workspace` | 2 | Lists elements |
| CLI command | `merge -workspace -layout [-view] -output` | 3 | Layout from workspace JSON |
| CLI command | `generate system-landscape -input -output` | 3 | Merges many workspaces, JSON input |
| CLI command | `c4rizer cache list\|clear` | 3 | c4rizer only; the network fetch cache (P3-612) |
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
| UI feature | Open the next level of detail from an element | 1 | EDIT-15 (double-click enters one level; a single click selects) |
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
| L27 | error | a line crosses a deployment node boundary that contains neither of its ends (P3-126) |
| L28 | warning | a vertex lies inside a box rectangle, or two consecutive vertices of one line are the same point (P3-309) |

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
- **CORE-27** Startup path: from file text to the routed scene of view
  `c3` of `example/` takes under 300 ms in a release build.
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

None.

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
  view scoped to it (for a container); a left double-click on an
  enterable box opens its view (container view of a system, component
  view of a container), exactly one level down and never twice for one
  double-click; a single left click selects any box, enterable or not
  (owner's ruling: "double click to enter next C4 level"); enterable
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
  screen px) selects; a left double-click enters an enterable box
  (EDIT-15); left drag on a box
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
- **EDIT-30** The first time a view is shown in a session, the camera
  fits the whole scene (drawing, title and key) into the canvas with a
  20px margin, never zooming above 100%; "Fit view" (F key and a
  toolbar button) does the same on demand; reloads keep zoom and pan
  (EDIT-1). Entering a view by clicking a box and going back up counts as
  showing it: coming back to a view already shown in this session
  restores the zoom and pan it had then.
- **EDIT-31** The canvas draws the scene on a page the colour of the
  export background (white in phase 1), so the canvas and the exported
  SVG/PNG look the same whatever the window theme. The area outside the page
  uses the app's panel colour, and selection marks, resize outlines and
  lint marks keep enough contrast to stay visible on the page.
- **EDIT-32** The canvas keeps at least 60% of the window width: side
  panels are resizable and collapsible, and at a window width under
  1000px they start collapsed.

### 4.5 Diagram exports

- **EXP-1** Exported SVG has no script and no external reference
  (tested by scanning the output) and names the bundled font first
  (CORE-30). The SVG does not embed the font: it names it in
  `font-family` with a `sans-serif` fallback and carries no
  `@font-face`.
- **EXP-2** PNG export of a view is its SVG rasterized at the requested
  width with the bundled font only (no system fonts).

### 4.6 Phase 2

Requirements grouped by ROADMAP phase 2 row, one testable sentence each
(rows' step 2, 2026-11-15). The parity table (section 2) lists every
feature they cover. Ids P2-1..P2-28 are the original stubs, kept; new
ids are allocated in one block per row so rows never collide:
place-quality P2-100.., dsl-breadth P2-120.., landscape-filtered
P2-300.., styles-themes P2-400.., compat-fixtures P2-500.., cli-breadth
P2-600.., editor-breadth P2-700.. `TODO(srs)` marks a hole listed in
`docs/deferred.md`.

place-quality

Row `place-quality` (phase 2), scope P2-26. Authority: SRS 4.6 P2-26;
phase-1 CORE-14, CORE-15, CORE-22, CORE-23, CORE-25, CORE-29, EDIT-4,
EDIT-20, EDIT-25; parity table rows `autoLayout` ("the arguments have no
effect"). Structurizr DSL reference, fetched 2026-11-15 from
https://docs.structurizr.com/dsl/language: `autoLayout [tb|bt|lr|rl]
[rankSeparation] [nodeSeparation]`, `tb` top to bottom is the default,
both separations default to 300px. The page names no layout engine and
says nothing about positions beyond direction and separations. "Example"
below means `tests/data/example/workspace.dsl` as checked in: no `at`
lines, no `size` lines, `autoLayout` on every view, view keys `C1`,
`C2`, `C3-serving`, `C3-startup`. It is built with
`pipeline::build_all`, which uses `Keep::default()`. A "kept box" is one
with an `at` line, a `Keep::moved` entry or a `Keep::previous` entry.
"Auto-placed" means every other box.

- **P2-26** (kept, umbrella) Automatic placement is layered: ranks
  follow the direction of relationships, order within a rank reduces
  crossings, people are on top and external elements at the sides; the
  row and column pitch grows where a label would not fit between two
  boxes. On the example, all four views have no "no room for label", no
  label over a box, and crossings and total line length no worse than
  phase 1 and lower on C3-startup. It is split into P2-100..P2-110
  below, and its numbers are in P2-107/P2-108. `TODO(srs)`: the owner
  approves those numbers before they freeze.
- **P2-100** In a view with no kept box, ranks run top to bottom
  (Structurizr's `tb` default). An *outward-facing external* is an
  auto-placed box with `PlanBox::external` that is not a person, has at
  least one drawn outgoing edge, and whose drawn outgoing edges all end
  on a person. Consider every drawn edge whose destination is neither a
  person nor an outward-facing external, and that lies on no directed
  cycle of the view's edges (counting only edges that end on neither).
  For each such edge, the source box's centre has a strictly smaller y
  than the destination box's centre.
- **P2-101** A view whose edges, not counting edges that end on a
  person, contain a directed cycle is still placed in full and
  terminates: every box gets a centre (CORE-22), the edges outside every
  cycle still satisfy P2-100, and two runs give equal centres (CORE-25).
- **P2-102** A view with no kept box and no person, made of sources `a`,
  `b` and destinations `c`, `d` declared in the order `a, b, d, c`, with
  edges `a -> c` and `b -> d`, is placed with zero straight-line
  crossings between edge ends.
- **P2-103** In a view with no kept box, no auto-placed box has a centre
  y smaller than that of any auto-placed person (CORE-23 "people on the
  top row"), including when an edge points from a non-person to a
  person. When the view has a person, every outward-facing external
  (P2-100) has the same centre y as the people.
- **P2-104** In a view with no kept box, sort the auto-placed boxes that
  share a centre y by centre x. They read as zero or more external boxes
  (`PlanBox::external`, people excluded), then every non-external box,
  then zero or more external boxes.
- **P2-105** In a view with no kept box, take every drawn edge with a
  label whose two boxes are in the same column on consecutive rows. The
  vertical gap in px between the two box rectangles is at least `max(30,
  h/2) + max(50, h/2)`. For every drawn edge with a label whose two
  boxes are on the same row in adjacent columns, the horizontal gap is
  at least `max(30, w/2) + max(50, w/2)`. Here `w` and `h` are that
  edge's `Label::width_px` and `Label::height_px` from
  `label::place_labels`. Source of the bounds: CORE-14, where the label
  centre is at least 30px along the line from its start and 50px from
  its arrowhead, and the label overlaps neither box.
- **P2-106** In a view with no kept box, the auto-placed centres lie on
  a lattice of rows and columns. Two consecutive rows are the smallest
  multiple of 10px apart that is at least 280px, keeps every two boxes
  of those rows that share a column at least 60px apart (L6), and meets
  P2-105 for every labelled edge between the two rows. Two adjacent
  columns follow the same rule with 340px and the horizontal bound of
  P2-105. So the pitch never drops below the CORE-23 minimum, and it
  grows only where a box size or a label needs it.
- **P2-107** Each of the four example views meets three conditions. No
  label has `Fit::NoRoom` (`Labels::no_room()` is empty, no L24). No
  label's px rectangle overlaps any box's px rectangle with positive
  area. The label rectangle is centred at `grid_to_px(centre)` with size
  `width_px` x `height_px`. The box rectangle is
  `PlanBox::rect_at(routes.centres[i])` scaled by `GRID_PX`. The lint
  report has no L26 finding. The phase-1 baseline was measured on the
  working tree on 2026-11-15. No-room labels: C1 0, C2 2, C3-serving 2,
  C3-startup 1. Label-over-box pairs: C1 0, C2 4, C3-serving 4,
  C3-startup 2. L26: 0 on every view.
- **P2-108** On each example view, routed crossings
  (`Routes::crossings`) and total line length are no higher than the
  phase-1 baseline. Both are strictly lower on C3-startup. No line is
  unroutable (`Routes::unroutable()` empty). Total line length is the
  sum over every segment of every routed line of `steps * 10_000`
  (axis-aligned) or `steps * 14_142` (45 degrees) milli-px, divided by
  1000 and truncated once. `steps` is the segment's larger grid delta.
  This follows the integer convention of label.rs. The baseline was
  measured on 2026-11-15. Crossings: C1 0, C2 0, C3-serving 1,
  C3-startup 9. Length: C1 691px, C2 1898px, C3-serving 7092px,
  C3-startup 18146px.
- **P2-109** In a view with a boundary and no kept box, no auto-placed
  box that is not a child of the scope overlaps the boundary rectangle
  (`ViewPlan::boundary_rect`, CORE-23, L26). This holds when such a box
  ranks between two ranks of children.
- **P2-110** The arguments of a view's `autoLayout` statement do not
  change auto-placed centres (parity table: "the arguments have no
  effect"). The same view with `autoLayout`, with `autoLayout lr 100
  50`, and with no `autoLayout` gives equal centres.
- **P2-111** On each example view, `pipeline::build_view` with
  `Keep::default()` takes under 1000 ms, and `place::place` alone under
  600 ms. Both are measured as the median of 5 runs after one warm-up,
  in a release build, checked by the release perf oracle
  (`perf_p2_111_*`, 5.2). Placement reads no clock and has no time cap
  (CORE-22, CORE-25, CORE-29).
- **P2-112** On example view C2, Push services (an outward-facing
  external, P2-100) has the same centre y as the people, sits at an end
  of that row (P2-104), and no line from Push services crosses the
  boundary rectangle more than once. On C3-serving, Push services has
  the same centre y as the people.
- **P2-113** On example views C2 and C3-serving, total line length
  (P2-108 metric) is at most 2840 px and 9400 px, routed crossings stay
  at 0 and 1, and L23 findings are at most 0 and 1. Measured 2026-11-17
  after the CORE-14 arrowhead fix: 2838 px and 9370 px.
- **P2-114** On example view C3-startup, routed crossings are at most 8,
  L23 findings at most 5, and total length at most 17733 px. No-room
  labels and labels over a box stay at 0 on every view (P2-107).
- **P2-115** `TODO(srs)` (owner): on example view C3-startup, routed
  crossings are at most 6, L23 findings at most 3, and total length at
  most 25 000 px, within P2-111. Evidence: a routed hill climb reached
  5 / 3 / 24665 px in 25 s (2026-11-17).

dsl-breadth

Sources: https://docs.structurizr.com/dsl/language, /dsl/basics,
/dsl/identifiers, /dsl/includes, /dsl/expressions, /dsl/archetypes,
/dsl/implied-relationships and /dsl/cookbook/groups/, fetched 2026-11-15
(the reviewer fetched basics, identifiers, includes, expressions,
archetypes, implied-relationships, groups and language again on the same
day). /dsl/text-blocks returned 404. The workspace-extension page has
only an overview, and the language page's "extends" text is used
instead. Where a page is silent, the item is marked TODO(srs). Existing
ids are kept, with their sentences tightened. New ids run from P2-120 to
P2-181. /dsl/basics answers the old P2-7 TODO(srs) on `!const`/`!var`
(P2-150..152), so the parity table needs a row for `!const`, `!var` and
`${NAME}`; `branding` is not a keyword in the current language reference
and is dropped from P2-7.

**Grammar and lexing**

- **P2-1** On `tests/data/dsl-breadth/grammar.dsl` (plus its include and
  base files), every keyword the Structurizr DSL language reference
  lists parses without error in each block where the reference allows
  it. Workspace: `name`, `description`, `properties`, `!identifiers`,
  `!docs`, `!adrs`, `model`, `views`, `configuration`, plus the
  `workspace extends` head. Model: `!identifiers`, `archetypes`,
  `group`, `person`, `softwareSystem`, `deploymentEnvironment`,
  `element`, `->`. Element blocks: `description`, `technology`, `tags`,
  `tag`, `url`, `properties`, `perspectives`, `group`, `!docs`, `!adrs`,
  `!components`, `->`. Directives: `!const`, `!var`, `!include`,
  `!script`, `!plugin`, `!element`, `!elements`, `!relationship`,
  `!relationships`, `!extend`, `!ref`, `!impliedRelationships`,
  `!decisions`. Views: `systemLandscape`, `systemContext`, `container`,
  `component`, `filtered`, `dynamic`, `deployment`, `custom`, `image`,
  `styles`, `theme`, `themes`, `terminology`, `properties`. View bodies:
  `include`, `exclude`, `autoLayout`, `default`, `animation`, `title`,
  `description`, `properties`. Styles: `light`, `dark`, `element`,
  `relationship`. Configuration: `scope`, `visibility`, `users`,
  `properties`. The compat-fixtures row tests the compat-corpus clause
  (P2-14).
- **P2-178** A keyword whose effect belongs to another phase-2 or
  phase-3 row is accepted with no error and kept byte for byte (DSL-11)
  until that row lands. This covers every style property and shape,
  `light`/`dark`, `theme`/`themes`, view expressions and relationship
  patterns in `include`/`exclude`, `default`, `animation` and the
  `Group`/`Group:Name` style tags.
- **P2-147** A `\` as the last character of a line continues the
  statement on the next line, and the model is the same as with the
  statement written on one line.
- **P2-148** A `"""` text block is accepted wherever a string value is.
  TODO(srs): the value rules (indent stripping, escapes); the reference
  page is missing (404).
- **P2-149** A positional string argument with no whitespace may be
  written unquoted (`person User`) and gives the same value as the
  quoted form.
- **P2-150** `!const NAME value`, with a name of only `a-zA-Z0-9-_.`,
  makes every later `${NAME}` inside any token, quoted strings included,
  take that value.
- **P2-151** `!var NAME value` may be redefined by a later `!var NAME
  other`, and each `${NAME}` takes the value in force at its line.
  TODO(srs): whether redefining a `!const` is an error. TODO(srs): what
  a DSL-10 edit does to a literal holding `${...}`.
- **P2-152** A `${NAME}` with no matching constant or variable takes the
  environment variable `NAME`, and with neither it is left as written.
  TODO(srs): which wins when a constant or variable and an environment
  variable share a name; the reference does not say.

**Deployment and dynamic parsed, not drawn (phase 3 draws them)**

- **P2-120** `deploymentEnvironment`, `deploymentGroup`,
  `deploymentNode`, `infrastructureNode`, `softwareSystemInstance`,
  `containerInstance`, `instanceOf`, `instances`, `healthCheck`, `-/>`
  and `group` inside `deploymentEnvironment` or `deploymentNode` all
  parse without error. An undeclared id they reference is a DSL-5 error,
  and an id they declare that is already in use is `DuplicateId`
  (identifiers are global, /dsl/identifiers). No element they declare is
  drawn on any static view or selected by `include *`.
- **P2-121** A `dynamic <*|system|container> [key] [desc]` view parses
  without error with its body steps: `[order:] <src> -> <dst> [desc]
  [tech]`, `[order:] <relationship id> [desc]`, and parallel `{ }`
  blocks. An undeclared id in its scope or steps is a DSL-5 error.
- **P2-179** A `deployment <*|system> <environment> [key] [desc]` view
  parses without error with `include`, `exclude`, `autoLayout`,
  `default`, `animation`, `title`, `description` and `properties` in its
  body. A scope or environment that is not declared is a DSL-5 error.
- **P2-2** A view whose kind has no drawer yet (`dynamic`;
  `systemLandscape`, `filtered`, `custom` and `image` until their rows
  land; `deployment` is drawn since deployment-view, P3-2) is in `Model::views` with its key and title.
  `pipeline::build_all` lists it in `AllViews::unsupported`, and the
  rest of the file builds as before with no error.
- **P2-122** For an unsupported view, `c4rizer render` and `c4rizer
  lint` print `== <key>: <title>` and then a line indented by three
  spaces (CLI-1) that contains `not yet supported`. They write no file
  for that view and add nothing to the exit code (CLI-2). TODO(srs): the
  exact line text.
- **P2-123** The editor's view list shows an unsupported view marked
  "not yet supported". Selecting it draws nothing and changes no file,
  and the editor never opens an unsupported view by itself: at startup
  and in the EDIT-27 fallback it opens the first supported view.
- **P2-180** The editor's "Export all views" (EDIT-7) skips unsupported
  views and writes no file for them.
- **P2-124** A `systemLandscape`, `dynamic` or `deployment` view with no
  key gets `SystemLandscape-NNN`, `Dynamic-NNN` or `Deployment-NNN` per
  DSL-7, numbered per kind in declaration order. TODO(srs): the
  generated key, or whether a key is required, for `custom`, `image` and
  `filtered` views; the reference states key generation only for the
  other kinds.

**Constructs accepted with no effect**

- **P2-3** Each of these is accepted and adds exactly one
  `Model::notices` entry naming the construct, its file and its line,
  with no other effect: `!script` (inline or file form), `!plugin`,
  `!components`, `healthCheck`, `configuration visibility`,
  `configuration users`, an `image` view with a `plantuml`, `mermaid` or
  `kroki` source, the `plantuml.*`, `mermaid.*` and `kroki.*` view-set
  properties, `!impliedRelationships <fqcn>`, and a `!docs`/`!adrs`
  importer FQCN.
- **P2-125** `c4rizer render` and `lint` print every notice before the
  first view, one line each naming the construct, file and line, and
  notices add nothing to the exit code. TODO(srs): the exact line
  format.
- **P2-126** The editor's findings panel (EDIT-6) shows every notice of
  the open file, naming the construct.

**Identifiers**

- **P2-145** An element declared without `<id> =` parses, and `include
  *` selects it as its kind allows. TODO(srs): how `at`, `size` and
  edits address such a box. EDIT-26 writes an `at` line for every box,
  and this box has no id. The app must not add an id itself unless the
  SRS says so, since editing must change text, layout and style only
  (EDIT-21).
- **P2-146** `r = a -> b` sets the relationship's key to `r`,
  `!relationship r` resolves it, and a relationship key declared twice
  is `DuplicateId` naming both lines. TODO(srs): whether a relationship
  id may equal an element id.
- **P2-144** In flat mode (the default, or `!identifiers flat`), a
  second element whose id is already used anywhere is `DuplicateId`,
  naming both lines.
- **P2-6** With `!identifiers hierarchical` in the workspace or `model`
  block, an element's full id is its parent's full id, then `.`, then
  its own id. So two `api` containers in systems `s1` and `s2` both
  parse and `Model::find` finds them as `s1.api` and `s2.api`, and two
  elements with the same full id are `DuplicateId`.
- **P2-142** Under hierarchical ids, a full id is accepted in
  relationships, `include`, `exclude`, `at`, `size`, `focus`, `hide`,
  `!element` and every `edit` element key, and `edit` writes the full
  id. TODO(srs): whether a bare id names a nested element from inside
  its parent's block.
- **P2-143** Under `!identifiers hierarchical`, relationship identifiers
  stay global: `r = a -> b` inside `s1` is found as `r`, not `s1.r`.
- **P2-131** A group adds no segment to a hierarchical id: a container
  `api` inside a group in `s1` is `s1.api`.
- **P2-177** Each of these is an error naming both lines: two people or
  two software systems with the same name, two containers with the same
  name in one software system, or two components with the same name in
  one container. Source: Structurizr for Java model rules, which no
  fetched DSL page states, so confirm (open question).

**Groups**

- **P2-4** `group "<name>" { ... }` is accepted in `model`, in a
  software system block and in a container block, and every element
  declared inside it has that group.
- **P2-127** A group holds only elements of its block's level: people
  and software systems in `model`, containers in a software system,
  components in a container. Any other element kind inside a group is an
  `InvalidNesting` error. TODO(srs): whether a custom `element` may sit
  in a model-level group.
- **P2-128** A `group "<name>"` line inside a component block sets that
  component's group (language reference, "group").
- **P2-129** With the `model` property `structurizr.groupSeparator` set
  to `/`, an element inside `group "A" { group "B" { ... } }` has the
  group `A/B`. TODO(srs): what a nested group with no separator set
  does.
- **P2-130** On every drawable view, a group with at least one drawn
  member draws as a dashed boundary (CORE-1: members' bounds plus 40px)
  around its drawn members, with the group's name as its name tag and no
  bracketed type. It sits inside its enclosing group's boundary, and
  also inside the scope boundary when its members are children of the
  scope, otherwise it does not overlap the scope boundary. TODO(srs):
  the name tag of a nested group (full path or last segment); the
  default colour (the reference says only "light grey"); whether
  `structurizr.groups` (P2-176) hides group boundaries.
- **P2-132** On `tests/data/dsl-breadth/groups.dsl` with no `at` lines,
  auto-placement leaves no non-member box overlapping any group
  rectangle, as CORE-23 requires for the scope boundary. TODO(srs):
  whether L26 also covers group rectangles, and its wording.

**!include and workspace extends**

- **P2-5** `!include <relative file>` inlines the file's statements
  where the directive stands, and the model is the same as with the text
  pasted there.
- **P2-133** `!include <relative directory>` inlines every DSL file in
  that directory. TODO(srs): order, recursion and which file extensions
  count. The reference says only "in the order the files are
  discovered".
- **P2-134** A relative include path that resolves outside the including
  file's directory and its subdirectories is an error naming the path.
- **P2-135** Each included file gets a `FileId` from 1, in the order
  files are first read, every span names its file, and printing each
  tree gives its file back byte for byte (DSL-11, DSL-16).
- **P2-136** A parse or model error in an included file carries that
  file's path, line and column (SRS 5.1), in the CLI report and in the
  editor (EDIT-11).
- **P2-137** A file whose includes form a cycle, a self-include
  included, stops with an error naming the file that closes the cycle.
- **P2-138** `!include` of a URL that is not `https://` is an error
  naming the URL. TODO(srs): fetch, cache and offline behaviour for
  HTTPS URLs (as P2-5 says).
- **P2-139** A DSL-10 text edit to a literal from an included file
  patches that file only, and the root file's bytes are unchanged (SRS
  P2-5). TODO(srs): where a DSL-13 per-box `tags` line goes for an
  element declared in an included file.
- **P2-140** A DSL-9 `at` or `size` edit of a view declared in the root
  file is written to the root file (SRS P2-5). TODO(srs): a view
  declared in an included file. SRS P2-5 says root file, but an `at`
  line must sit inside the view block, so the two rules conflict.
- **P2-141** The editor watches every included file: a change to any of
  them reloads as EDIT-1 does, a save by rename is seen as EDIT-23 says,
  and the app's own write to one does not reload, as EDIT-22 says.
- **P2-16** `workspace extends <relative DSL file>` loads the base
  workspace and applies this file on top, and the base's elements,
  relationships and views are in the model. TODO(srs): URL bases (same
  rules as P2-5); JSON bases, which the reference allows; which file a
  text edit to a base literal goes to.
- **P2-164** In an extending workspace, a relationship or view may
  reference an id declared in the base file.
- **P2-165** In an extending workspace, `!element <base id> { tags "X"
  }` adds tag `X` to the base element.

**Directives that modify the model**

- **P2-17** `!element <id> { ... }` finds an existing element and adds
  the tags, properties, child elements and relationships of its block.
- **P2-166** `!elements <expression> { ... }` applies its `->`, `tag`,
  `tags`, `url`, `properties` and `perspectives` children to every
  element the expression selects. This holds for each element-expression
  form of /dsl/expressions and for `&&` and `||`.
- **P2-167** `!relationship <id> { ... }` adds `tag`, `tags`, `url`,
  `properties` and `perspectives` to that relationship.
- **P2-168** `!relationships <expression> { ... }` applies `tag`,
  `tags`, `url`, `properties` and `perspectives` to every relationship
  the expression selects. This holds for each relationship-expression
  form of /dsl/expressions and for `&&` and `||`.
- **P2-169** `!extend` and `!ref` behave as `!element`, or as
  `!relationship` when the id names a relationship. TODO(srs): `<id> =
  !element <canonical name>` and `<id> = !relationship <canonical
  name>`, which the reference gives only for JSON-based workspaces.
- **P2-170** `element <name> [metadata] [description] [tags] { ... }` in
  `model` declares a custom element with only the implicit tag `Element`
  and its metadata kept, and it accepts `description`, `tags`, `url`,
  `properties`, `perspectives` and `->` children. TODO(srs): whether
  `include *` on a context, container or component view may select a
  custom element. The reference says only that a custom view takes
  custom elements only.

**Body statements and settings**

- **P2-18** `tag "<name>"` in an element or relationship block adds that
  one tag after the tags before it (DSL-15 order).
- **P2-171** `url <url>` in an element or relationship block sets that
  item's URL.
- **P2-172** `properties { <name> <value> ... }` in the workspace,
  `model`, `configuration`, an element, a relationship, a view or
  `views` stores its pairs in file order on that owner.
- **P2-173** `perspectives` in an element or relationship block stores
  each perspective's name, description, value and URL. Both forms work:
  the one-line `<name> <description> [value]` form and the `perspective
  <name> { description, value, url }` block form.
- **P2-174** `name "<text>"` and `description "<text>"` lines in the
  workspace block set the workspace name and description.
- **P2-175** `configuration { scope landscape|softwaresystem|none }`
  stores the scope. TODO(srs): the validation the scope applies.
- **P2-176** The `structurizr.*` properties of `views` (`locale`,
  `timezone`, `sort`, `tooltips`, `title`, `description`, `metadata`,
  `groups`) are stored. TODO(srs): one requirement per property for its
  effect.

**Implied relationships, archetypes, docs, terminology (P2-7 split)**

- **P2-7** With `!impliedRelationships false`, no lifted line is drawn
  (CORE-5 lifting off), and `include *` counts only relationships
  declared on the element itself. TODO(srs): what a directive placed
  between relationships does; the reference does not say whether it
  applies only to later relationships.
- **P2-153** `!impliedRelationships true` behaves exactly as no
  directive, and both behave as CORE-5 (the reference names
  CreateImpliedRelationshipsUnlessAnyRelationshipExistsStrategy for
  `true`).
- **P2-154** `!impliedRelationships <fqcn>` behaves as P2-153 and adds a
  notice (P2-3), as the parity table says ("parsed, no effect").
- **P2-155** With `archetypes { service = container { technology "Go";
  tags "Service" } }` in `model`, `x = service "Name"` declares a
  container with technology `Go` and tag `Service`, and a positional or
  body `description`/`technology` overrides the default.
- **P2-156** An archetype based on another archetype (`api = service {
  technology "Go REST" }`) takes the base's description, technology,
  properties and perspectives defaults and overrides those it sets
  itself. TODO(srs): how the two archetypes' tags combine.
- **P2-157** With `sync = -> { tags "Synchronous" }` and `https =
  --sync-> { technology "HTTPS" }`, `a --https-> b "d"` declares a
  relationship with tag `Synchronous`, technology `HTTPS` and
  description `d`.
- **P2-158** An element archetype whose base is not one of `person`,
  `softwareSystem`, `container`, `component`, `deploymentNode`,
  `infrastructureNode`, `group` or `element` is an error naming the
  base. A `--name->` naming no relationship archetype is an error naming
  it. TODO(srs): the tag order of an element declared from an archetype
  (DSL-15).
- **P2-181** An archetype's technology default is applied only to
  containers and components, and its metadata default only to custom
  elements. TODO(srs): whether a technology on another base is an error
  or ignored.
- **P2-159** `!docs <path> [fqcn]` in the workspace or a software
  system, container or component block records the path on that owner.
  The language page's block lists include component, and its prose names
  only the first three. TODO(srs): whether phase 2 reads or checks the
  files.
- **P2-160** `!adrs <path> [type|fqcn]` and `!decisions` record the path
  and the importer type on their owner, in the same blocks as `!docs`.
- **P2-161** `terminology { softwareSystem "Service" }`, and likewise
  `person`, `container` and `component`, replaces that type word on box
  type lines and boundary name tags (`[Service]`). TODO(srs): the CORE-2
  `External Software System` type word under terminology.
  `deploymentNode` and `infrastructureNode` are stored and take effect
  in phase 3.
- **P2-162** `terminology { relationship "Link" }` makes the key's line
  entries read `Link` instead of `Relationship` (CORE-1).
- **P2-163** `terminology { metadata round|curly|angle|none }` writes
  the box type line as `(…)`, `{…}`, `<…>` or with no brackets, and
  `square` is the default `[…]`. TODO(srs): the glyphs for
  `double-angle`.

landscape-filtered

Row `landscape-filtered` (phase 2) owns P2-8, P2-9, P2-19 and P2-20.
Each existing id keeps the part the SRS already settles, and each TODO
part becomes a new id from P2-300. The authority is the Structurizr DSL
reference, fetched 2026-11-15: /dsl/language (the permitted-children
table, include, exclude, image), /dsl/expressions, and the cookbook
pages /dsl/cookbook/filtered-view/, /dsl/cookbook/custom-view/ and
/dsl/cookbook/image-view/. Every fetch succeeded. Where those pages are
silent, the sentence says so and carries TODO(srs). A claim that rests
on memory of the Structurizr implementation rather than on the reference
is marked "(memory)" and stays TODO(srs).

**System landscape (P2-8)**

- **P2-8** A `systemLandscape ["key"] ["description"]` view with
  `include *` draws every person and software system of the model as a
  box, with no scope boundary and no name tag.
  - Its relationships are drawn after lifting (CORE-5) and routed and
    labelled under the rules every view follows (CORE-6..15).
  - CORE-1 title, subtitle and key are unchanged.
- **P2-300** `systemLandscape` takes no scope, then an optional key and
  an optional description.
  - Its body accepts `include`, `exclude`, `autoLayout`, `default`,
    `title`, `description` and `properties` (the reference's permitted
    children), plus the c4rizer view extensions of the SRS 4.1 table
    (`subtitle`, `technology`, `bus`, `labelWidth`, `maxCrossings`,
    `focus`, `hide`, `at`, `size`).
  - `animation` is parsed and kept byte for byte (P2-1), draws nothing
    until phase 3 (parity row: phase 3), and shows the P2-3 notice.
- **P2-301** A `systemLandscape` view with no key gets
  `SystemLandscape-NNN`. Keys are numbered per kind in declaration
  order, skipping keys already taken (the DSL-7 rule applied to the new
  kind).
- **P2-302** On a system landscape view, `include *` selects every
  person and every software system of the model, connected or not
  (reference: the wildcard on a landscape "adds all people and
  systems"). An element named in `include <id>` is kept whatever its
  kind, as in phase 1.
- **P2-303** On a system landscape, custom or image view, lint runs
  every rule of the lint table that is not tied to a view kind or to the
  whole file (L1..L14, L23..L26), and none of L19..L21.
- **P2-304** TODO(srs): which of the C1 rules L15..L18 apply to a system
  landscape view. The P2-8 stub said "same lint rules as C1", but L17
  (more than 8 boxes) works against the purpose of a landscape.
- **P2-305** In a system landscape view, a double-click on an enterable box
  opens its view exactly as EDIT-15 says, and the breadcrumb is the
  workspace crumb alone, since a landscape has no scope (EDIT-16).
  TODO(srs): whether the workspace crumb (EDIT-16) should open the first
  declared system landscape view before the first system context view.

**Filtered view (P2-9)**

- **P2-9** A `filtered <baseKey> include|exclude "<tags>" ["key"]
  ["description"]` view draws its base view, keeping (`include`) or
  dropping (`exclude`) the elements and relationships that carry those
  tags. Every box takes the base view's `at` and `size` lines.
- **P2-306** A `baseKey` that names no view is an error naming the key
  (as DSL-5).
  - TODO(srs): base views of other kinds. From memory, Structurizr
    accepts only a `systemLandscape`, `systemContext`, `container` or
    `component` view, or a deployment view (P3-130), as the base. The reference pages read do not say.
    If confirmed, any other kind (filtered, custom, image) is an error
    naming the key and its kind.
  - TODO(srs): whether the base view may be declared after the filtered
    view.
- **P2-307** A filtered view's body accepts `default`, `title`,
  `description` and `properties` (reference). `include`, `exclude`,
  `autoLayout` and `animation` there are an error naming the construct.
  - TODO(srs): whether the c4rizer extensions (`at`, `size`, `focus`,
    `hide`, `subtitle`, `technology`, `bus`, `labelWidth`,
    `maxCrossings`) are accepted in a filtered view and override the
    base view's.
- **P2-308** The `<tags>` argument is comma-separated and trimmed, like
  `tags`.
  - In `include` mode, a box of the base view is kept when its element
    has at least one of the tags.
  - In `exclude` mode, a box is dropped when its element has at least
    one of them.
  - Implicit tags (`Element` and the kind tag, DSL-15) count.
  - Source: the cookbook keeps "all elements/relationships that are
    tagged `Tag 1`, `Tag 2`, or `Relationship`".
- **P2-309** A line of the base view is kept only when both of its boxes
  are kept.
  - In `include` mode, its relationship must also carry at least one of
    the tags.
  - In `exclude` mode, its relationship must carry none of them.
  - The implicit `Relationship` tag counts, so `include
    "Element,Relationship"` draws the base view unchanged (cookbook
    `landscape-all`).
- **P2-310** TODO(srs): a lifted line can merge several relationships
  (CORE-5). Whose tags decide P2-309 for it: the winner's, any merged
  relationship's, or each relationship's on its own with the survivors
  merged again?
- **P2-311** A filtered view's drawn title is its own `title`, else its
  own description (DSL-6), and L8 applies to it. TODO(srs): whether it
  takes the base view's c4rizer options (`technology`, `bus`,
  `labelWidth`, `maxCrossings`, `focus`, `hide`, `subtitle`) or the
  defaults.
- **P2-312** TODO(srs): what happens to a box with no `at` line in the
  base view. Either it takes the centre it gets in the base view's scene
  (shared layout; that is Structurizr's renderer behaviour from memory,
  and the reference pages do not say), or it is auto-placed within the
  filtered view alone.
- **P2-313** TODO(srs): moving or resizing a box in a filtered view.
  Either it writes the base view's `at`/`size` line (DSL-9, EDIT-26),
  which follows from P2-9 sharing the lines, or the move is refused with
  a status-bar message. Depends on P2-307.
- **P2-314** A view that is the base of at least one filtered view is
  left out of the editor's view list. This amends EDIT-8 (reference: the
  base view "no longer shows in the list of viewable diagrams. This is
  by design.").
  - TODO(srs): whether `c4rizer render` still writes it (CLI-1 says one
    SVG per view).
  - TODO(srs): whether EDIT-15 and EDIT-16 may still open it.
- **P2-315** TODO(srs): the key generated for a filtered, custom or
  image view with no key. The reference says only that generated keys
  "are not guaranteed to be stable". The DSL-7 pattern would give
  `Filtered-001`, `Custom-001` and `Image-001`.
- **P2-342** TODO(srs): which kind-scoped lint rules run on a filtered
  view. The choices are the base view kind's rules (L15..L21) or only
  the rules P2-303 lists.

**Custom and image views (P2-19)**

- **P2-19** A `custom` view draws custom elements, and an `image` view
  with a local file source shows that PNG or SVG.
- **P2-316** `custom ["key"] ["title"] ["description"]` takes the title
  as its second argument (reference).
  - Its body accepts `include`, `exclude`, `autoLayout`, `default`,
    `title`, `description` and `properties`, plus the c4rizer view
    extensions as P2-300 lists them.
  - `animation` is handled as in P2-300.
- **P2-317** Only custom elements (`element` declarations, P2-17) may be
  included in a custom view. `include <id>` naming any other element is
  an error naming the id (reference: "Only custom elements are permitted
  to be included on a custom view").
- **P2-318** `include *` on a custom view selects every custom element
  of the model and draws the relationships between them. Source: the
  cookbook custom-view example uses `include *` with two custom elements
  and one relationship; the language page does not list a custom-view
  wildcard.
- **P2-319** TODO(srs): the default box size and type line of a custom
  element. CORE-2 lists only the four C4 kinds.
- **P2-320** `image <*|element> ["key"] { image <file> }` with a local
  `.png` or `.svg` file builds a scene holding that image, the view's
  title and subtitle, and an empty key.
  - Canvas, SVG export and PNG export show the same image.
  - Besides the source line, the body accepts `default`, `title`,
    `description` and `properties` (reference).
- **P2-343** An `image <element>` view whose element identifier is
  undeclared is an error naming the id (DSL-5).
- **P2-340** The SVG export of an image view still meets EXP-1: no
  script and no external reference, with the image carried inside the
  file. TODO(srs): what happens to an SVG source that holds a script or
  an external reference (rejected with an error naming the file,
  stripped, or rasterized).
- **P2-321** An image view whose source is `plantuml` or `mermaid` draws
  a placeholder naming the source kind and the file, URL or view key as
  written (parity table: never, placeholder shown). A `kroki <format>`
  source is parsed, draws nothing and shows the P2-3 notice (parity
  table: never, no effect).
- **P2-322** TODO(srs), image views (one item each):
  - the base path for `image <file>`: the diagram file's directory, and
    whether the path must stay under it as an `!include` path must;
  - the error when the file is missing or is not PNG/SVG;
  - the drawn size and scaling of the image;
  - the `<url>` form: fetch, cache, offline;
  - what `*` means compared with an element identifier (the reference is
    silent);
  - whether an image view scoped to an element makes its box enterable
    (EDIT-15), as the cookbook's "level 4" component example suggests.

**View statements beyond phase 1 (P2-20)**

- **P2-20** `include *?`, element and relationship expressions with
  `&&`/`||`, relationship patterns in `exclude`, `default` and view
  `properties` work as in Structurizr. Each form has its own id below.
- **P2-323** `include *?` on a system context, container or component
  view selects the same elements as `*`, but draws only the
  relationships to or from the scoped element (reference): the scoped
  system (C1), or the scope's children (C2: its containers; C3: its
  components). TODO(srs): whether `*?` on any other view kind is an
  error. The reference only says it "is available for" these three
  kinds.
- **P2-324** `include ->x`, `x->` and `->x->`, and the forms
  `element==->x`, `element==x->` and `element==->x->`, add x plus every
  element with a relationship to x, from x, or either (reference: "the
  element(s) plus afferent/efferent couplings"; x may itself be an
  expression).
  - In `exclude`, the same expressions remove those elements.
  - A relationship of a nested element counts for its ancestors, as with
    phase-1 `include *`.
- **P2-325** TODO(srs): when a coupled element from P2-324 is nested, at
  which level it is drawn (itself, or its ancestor at the view's level),
  and whether such an element is kept when the view kind does not
  normally show it.
- **P2-326** `element.type==<T>` matches the elements of type T, for T
  in `Person`, `SoftwareSystem`, `Container`, `Component` and `Custom`
  (spelled as the reference spells them). The deployment types
  (`DeploymentNode`, `InfrastructureNode`, `SoftwareSystemInstance`,
  `ContainerInstance`) belong to phase 3.
- **P2-327** `element.parent==<id>` matches the elements whose parent is
  `<id>`.
- **P2-328** `element.tag==<t>[,t]` matches the elements that have every
  listed tag. `element.tag!=<t>[,t]` matches those that do not have
  every listed tag (reference: "all of the specified tags").
- **P2-329** `element.technology==<t>` matches the elements whose
  technology is t, and `element.technology!=<t>` matches those whose
  technology is not t.
- **P2-330** `element.properties[<name>]==<value>` matches the elements
  whose property `<name>` is set to that value. It needs element
  properties (P2-18).
- **P2-331** `element.group==<name>` matches the elements in that group.
  It needs groups (P2-4).
- **P2-332** In `exclude`, each of these relationship expressions
  removes the drawn lines whose relationship matches and keeps every
  box:
  - `*->*`, `<id>->*`, `*-><id>`, `relationship==*`,
    `relationship==*->*`;
  - `relationship.tag==` and `relationship.tag!=` (every listed tag);
  - `relationship.properties[<n>]==<v>` (needs P2-18);
  - `relationship.source==<id>`, `relationship.destination==<id>`;
  - `relationship==<id>->*`, `relationship==*-><id>`,
    `relationship==<a>-><b>`.
- **P2-333** `exclude "<*|id|expression> -> <*|id|expression>"` removes
  every drawn line whose source box matches the left side and whose
  destination box matches the right side. It acts only on elements in
  the view (reference).
- **P2-334** A relationship expression in `include` adds no box. It
  matches only relationships whose two ends are already in the view
  (reference: "Relationship expressions only match elements already in
  the view").
- **P2-341** `include <r>` or `exclude <r>`, where `r` names a
  relationship (`r = a -> b`, dsl-breadth), adds or removes that
  relationship's line. The line is drawn only when both of its ends are
  in the view (reference: include and exclude accept a relationship
  identifier).
- **P2-335** Two expressions joined by `&&` match what both match, and
  joined by `||` they match what either matches (reference example:
  `"element.type==Container && element.parent==abc"`).
- **P2-336** TODO(srs): more than two operands, `&&`/`||` precedence
  when mixed, and parentheses. The reference covers only "two
  expressions" and points to scripts for anything more complex.
- **P2-337** An expression that contains whitespace must be written in
  double quotes (reference). Unquoted, it is an error naming the token,
  never read silently as something else (DSL-4).
- **P2-338** A `default` line in a view makes it the view the editor
  shows when no last-viewed view applies. It replaces "else the first
  declared" in EDIT-13. TODO(srs): what happens when two or more views
  carry `default`.
- **P2-339** A view `properties { <name> <value> }` block is parsed,
  kept byte for byte (DSL-11), and has no effect on the scene.

styles-themes

Sources fetched 2026-11-15: https://docs.structurizr.com/dsl/language
(styles, light, dark, element style, relationship style, theme, themes),
https://docs.structurizr.com/server/diagrams/notation (defaults,
Boundary/Group tags, which properties apply where, icons) and the
cookbook page relationship-styles (`dashed`). Taken from the draft
without re-fetching: the themes page (a theme is a folder holding
`theme.json` and its icons) and export/png-and-svg (`-mode`, default
light). From memory, not from a fetched page, and marked where used: the
Structurizr theme-JSON key names. Existing ids are kept. The TODO stubs
are split into P2-400..P2-439 and P2-440, one testable sentence each.

**Element styles (P2-10)**

- **P2-10** In an `element "<tag>"` block (top-level in `styles` or
  inside `light`/`dark`), each of `shape`, `icon`, `width`, `height`,
  `background`, `color`, `colour`, `stroke`, `strokeWidth`, `fontSize`,
  `border`, `opacity`, `metadata`, `description` and `properties { name
  value }` is parsed into the style with its value span, and any other
  word is still a DSL-4 error naming it.
- **P2-400** `colour` and `color` are one property (box text colour), so
  a block that sets both is the same duplicate-property error as a block
  that sets `color` twice.
- **P2-401** Wherever a `#rrggbb` colour is accepted (element
  `background`, `color`/`colour` and `stroke`; relationship
  `color`/`colour`), a CSS/HTML colour name in any letter case resolves
  to its CSS hex value, and any other word is an invalid-value error.
- **P2-402** A resolved `width W` or `height H` replaces that dimension
  of the CORE-2 kind default, each on its own, and a view's `size` line
  for the box still wins over both. `TODO(srs)`: a value that is not a
  multiple of 20 (`size` requires multiples of 20; proposal: round up),
  and whether Structurizr numbers are taken literally in px
  (Structurizr's default box is 450x300 at 24px text, c4rizer's is
  240x140 at 14px).
- **P2-403** `stroke <colour>` sets the box outline colour, and with no
  `stroke` the outline is the fill darkened to 72%, as in phase 1 (the
  reference's "darker version of the background").
- **P2-404** `strokeWidth N` draws the box outline N px wide, N outside
  1..10 is an invalid-value error naming N, and with no `strokeWidth`
  the outline stays 1px as in phase 1 (Structurizr's default is 2px).
- **P2-405** `border solid|dashed|dotted` strokes the box outline with
  no dash array, with a dash, or with a dash shorter than the dashed
  one, and the default is `solid`.
- **P2-406** `opacity N` (N outside 0..100 is an invalid-value error)
  draws the whole box (fill, outline, texts, icon) at N/100 opacity in
  canvas, SVG and PNG, and at 100 (the default) the SVG carries no
  opacity attribute.
- **P2-407** `fontSize N` draws the box name at N px, and wrapping, the
  text-fit check (L25) and the EDIT-18 minimum size use that size.
  `TODO(srs)`: how the type line and description scale with N (proposal:
  round(N x 11 / 14) px on an N px pitch).
- **P2-408** `metadata false` leaves out the box's `[type: technology]`
  line and `description false` leaves out its description (both default
  `true`), and the box's remaining texts still fit inside it.
- **P2-409** `icon <file>` with a local PNG or JPG path, relative to the
  diagram file's folder, draws that image inside the box rectangle,
  clear of its texts, the same in canvas, SVG and PNG. `TODO(srs)`: icon
  size and position (the reference says top, left or bottom, but no DSL
  property chooses), URL and base64 data-URI icon values (the reference
  allows both), SVG icons (the reference contradicts itself), and
  whether a missing file is a parse error or a lint warning. In the SVG,
  the icon is an `<image>` with a `data:` URI. That is no external
  reference, so the SRS text of EXP-1 allows it, but the export module's
  own EXP-1 test bans `<image>` and `href` and has to be relaxed.
- **P2-410** `properties { name value }` in an element or relationship
  style is parsed, kept byte for byte, and changes no scene.
- **P2-411** Element styles for `Boundary` and then
  `Boundary:SoftwareSystem` (container views) or `Boundary:Container`
  (component views) apply, in that tag order, to the CORE-1 boundary's
  `background` (fill), `stroke`, `strokeWidth`, `color` and `fontSize`
  (name tag), `border`, `opacity` and `metadata` (`false` drops the
  bracketed type from the name tag), and a property no such block sets
  keeps its phase-1 value (dashed 8/6, 1px, #666666, no fill). `shape`,
  `width`, `height` and `description` are ignored. `TODO(srs)`: a
  boundary `icon` (the reference puts it in the bottom-left corner).
- **P2-412** Element styles for `Group` and then `Group:<name>` apply to
  group boundaries (P2-4, row dsl-breadth) with the P2-411 properties
  except `metadata`.
- **P2-440** `element "Boundary:Enterprise"` is accepted, changes no
  scene and shows the P2-3 notice naming it (the reference marks it
  deprecated).

**Shapes (P2-11)**

- **P2-11** `shape` accepts the 19 reference shapes in any letter case
  (Box, RoundedBox, Circle, Ellipse, Hexagon, Diamond, Cylinder, Bucket,
  Pipe, Person, Robot, Folder, WebBrowser, Window, Terminal, Shell,
  MobileDevicePortrait, MobileDeviceLandscape, Component), and each
  draws as its own scene outline, which canvas, SVG and PNG all take
  from the scene.
- **P2-413** For every shape, the outline lies inside the box rectangle
  and the box's texts lie inside the outline's text area, which is the
  area L25 measures fit against. `TODO(srs)`: exact proportions per
  shape (the reference gives none).
- **P2-414** For every shape, every line still meets CORE-6..9, measured
  on the box rectangle. `TODO(srs)`: port rules per shape (CORE-9 covers
  only box, person and cylinder), and whether a line ends at its
  rectangle port or on an outline that leaves the rectangle side
  (Circle, Ellipse, Hexagon, Diamond, Robot). The second puts the end
  point off CORE-6's 10px grid.
- **P2-415** A RoundedBox has a larger corner radius than a Box.
  `TODO(srs)`: the reference's Box has square corners, but the phase-1
  Box has a 6px radius, which is in the golden SVGs.
- **P2-416** A box entry's key swatch draws that entry's shape outline.
- **P2-417** The editor style panel (EDIT-17) offers all 19 shapes, and
  `edit` writes the chosen one in the reference's spelling (for example
  `shape MobileDevicePortrait`).

**Relationship styles (P2-12)**

- **P2-12** A `relationship "<tag>"` block (top-level in `styles` or
  inside `light`/`dark`) accepts `thickness`, `color`, `colour`,
  `style`, `dashed`, `routing`, `jump`, `fontSize`, `width`, `position`,
  `opacity` and `properties`, and each drawn line takes the style of its
  winning relationship (CORE-5), resolved per property over
  `Relationship` and then the declared tags by the DSL-15 rules.
- **P2-418** `thickness N` draws the line and its chevrons N px wide,
  and the default stays 2px (CORE-2). `TODO(srs)`: whether the
  arrowhead, the join dot and the 20px lane pitch scale with N.
- **P2-419** `color`/`colour` gives the line, its chevrons, its tip and
  its label text that colour in place of the destination box's fill
  (overrides CORE-3).
- **P2-420** `style solid|dashed|dotted` draws the line's polyline
  solid, dashed or dotted, `dashed true|false` (the cookbook form,
  absent from the language page) means `style dashed`/`style solid`, and
  the default stays solid (CORE-10) although Structurizr's default is
  dashed.
- **P2-421** `routing Direct|Orthogonal|Curved` is parsed into the
  resolved relationship style and changes no scene. `TODO(srs)`: what
  each value means under the circuit-board router (the parity table says
  it "maps onto the c4rizer router"). Both example fixtures style
  `Relationship` with `routing Orthogonal`.
- **P2-422** With `jump true`, each crossing on that line is drawn as a
  hop at the crossing point in canvas, SVG and PNG, and L4's crossing
  count does not change. `TODO(srs)`: which line hops when both lines
  have `jump true`, and the hop size.
- **P2-423** A relationship `fontSize N` draws that line's label at N
  px, and its label size, placement clearances and L25 fit use that
  size. `TODO(srs)`: the line pitch (proposal: N + 2, as 11/13 in phase
  1).
- **P2-424** A relationship `width W` wraps that line's label to W px of
  text width, measured with the CORE-30 width table, in place of
  `labelWidth` characters.
- **P2-425** A relationship `position P` (P outside 0..100 is an
  invalid-value error) orders that label's candidate centres by distance
  from P% of the line's length instead of 50%, with CORE-14's gaps and
  clearances kept, and with no `position` the placement is exactly as in
  phase 1.
- **P2-426** A relationship `opacity N` (N outside 0..100 is an
  invalid-value error) draws the line, its chevrons, tip and label at
  N/100 opacity, and at 100 the SVG carries no opacity attribute.

**Themes (P2-13)**

- **P2-13** `theme <x>` and `themes <x> [y ...]` inside `views` are
  parsed, and a theme's styles resolve as if written before the file's
  first style block, in theme order: for one tag, the file's own block
  beats any theme's and a later theme beats an earlier one, while across
  tags the DSL-15 tag order decides.
- **P2-427** A theme given as a local file path, relative to the diagram
  file's folder, is read as Structurizr theme JSON (`elements` and
  `relationships` arrays of objects, each with `tag` and Structurizr
  style keys; key names from memory). Unknown keys are ignored, nothing
  is ever written to the theme file, and a missing file or malformed
  JSON is a parse error naming the file and saying what to fix (5.1).
- **P2-428** A relative `icon` path in a theme resolves against the
  theme file's folder.
- **P2-429** A theme given by name or by URL is accepted, changes no
  scene and shows the P2-3 notice naming it. `TODO(srs)`: bundled
  copies, a local cache or a fetch (the HLD has no network dependency;
  the reference says named themes must be installed).

**Light and dark (P2-21)**

- **P2-21** `light { }` and `dark { }` inside `styles` accept only
  `element` and `relationship` blocks, and anything else inside them is
  a DSL-4 error.
- **P2-430** Light mode resolves styles over the top-level and `light`
  blocks, dark mode over the top-level and `dark` blocks, and a file
  with neither kind of block resolves the same style for every element
  and relationship in both modes. `TODO(srs)`: for one tag, whether a
  mode block beats a top-level block wherever each is written (proposal:
  the mode block wins), and whether theme styles apply in both modes.
- **P2-431** `TODO(srs)`: the dark-mode defaults for diagram background,
  unstyled box and line colours, title, subtitle, key and boundary (the
  reference only says "colour scheme").
- **P2-432** `c4rizer render FILE -o DIR --mode light|dark` writes the
  SVGs (and the PNGs with `--png`) in that mode, the default is `light`
  (Structurizr `-mode`), and any other value is a usage error (exit 2).
- **P2-433** The editor has a light/dark switch that rebuilds the shown
  view in the chosen mode without writing to the file, and the editor's
  export uses the shown mode.

**Key (P2-27)**

- **P2-27** Each drawn line belongs to the last tag of its winning
  relationship, in DSL-15 tag order, that has a relationship style block
  and is not `Relationship`, and the key has one line entry per such
  tag, named by the tag, plus one entry named `Relationship` for all the
  other drawn lines. `TODO(srs)`: whether a line ending in a join dot
  (CORE-8) keeps its own entry.
- **P2-434** A named line entry's swatch is drawn with that style's
  resolved colour, thickness and dash. `TODO(srs)`: the swatch colour of
  the `Relationship` entry when CORE-3 gives its lines different colours
  (proposal: #707070).
- **P2-435** A box look in the key is (shape, fill, text colour, stroke
  colour, stroke width, border), so boxes that differ only in stroke or
  border get separate entries (CORE-1: "every shape, colour and line
  style drawn"). `TODO(srs)`: whether opacity and icon split entries.

**Cross-cutting**

- **P2-436** A fixture that uses every element style property, every
  shape and every relationship style property gives an editor draw list
  with the same items, outlines, strokes, dashes and opacities as its
  SVG, and its PNG is that SVG rasterized (EDIT-12, EXP-2).
- **P2-437** A file with no phase-2 style construct gives the same scene
  as in phase 1 except for the key's line entries, which P2-27
  collapses. The four golden SVGs are regenerated once for that change,
  the owner re-approves them (CLI-5 (2)), and they are byte-identical
  after that.
- **P2-438** A file using every construct of this row (all properties,
  all shapes, `light`/`dark`, `theme`/`themes`, colour names, `colour`,
  `dashed`) round-trips byte-identical (DSL-11).
- **P2-439** A DSL-13 style edit changes only the edited property's
  line. On a block that uses `colour`, a text-colour edit rewrites that
  line and adds no `color` line. A theme style is never edited: an `all
  <tag>` edit on a tag styled only by a theme adds a block for that tag
  to the file's own `styles`.

compat-fixtures

Sources: docs/srs.md sections 2 (parity table), 4.1, 4.6 and 5; phase-1
code on dev (model.rs, edit.rs, view.rs, pipeline.rs, lint.rs, cli
main.rs); and the Structurizr reference. The doc pages
docs.structurizr.com/json and the structurizr/json schema returned 404
when the draft was written. Where a page was missing, the behaviour
comes from the reference implementation's source:
github.com/structurizr/structurizr (Apache-2.0) at commit
ad34036c5a1227535fc9adea9a0f97de5962189d, files
DefaultLayoutMergeStrategy.java, SVGReader.java,
IdentifiersRegister.java, StructurizrDslParser.java and DslTests.java.
Anything taken from memory is marked as such.

- **P2-14** (kept, made precise) The oracle tests run over every file
  that `tests/data/structurizr/MANIFEST` marks `root`, plus
  `docs/workspace.dsl` read where it lives. They check three things:
  each file parses with zero errors, with `!include` resolved per P2-5;
  `pipeline::build_all` gives a `ViewBuild` for every view that P2-2
  does not mark "not yet supported"; and `export::svg` of each of those
  builds' scenes is a non-empty SVG document.
- **P2-500** The corpus is the tree
  `structurizr-dsl/src/test/resources/dsl/` of
  https://github.com/structurizr/structurizr at commit
  `ad34036c5a1227535fc9adea9a0f97de5962189d`, plus
  `structurizr-export/src/test/resources/big-bank-plc.json`, copied into
  `tests/data/structurizr/` at their relative paths. `SOURCE` lists
  every copied file with its git blob id at that commit. The test
  `p2_500_corpus_pinned_source` recomputes each file's git blob SHA-1
  with a helper inside the test and fails on any mismatch, missing file
  or unlisted file.
- **P2-501** `tests/data/structurizr/MANIFEST` has exactly one line per
  `.dsl` file in the copied tree. Each line marks the file `root`,
  `fragment` (read only through another file's `!include` or `workspace
  extends`), or `excluded <reason>`, and the reason is one of these:
  - `rejected-by-structurizr`: Structurizr's own `DslTests` at the
    pinned commit expect a parse failure.
  - `needs-network`: an `!include`, `workspace extends`, theme or image
    source that is a URL. This holds until the P2-5/P2-16 URL
    `TODO(srs)` is settled.
  - `needs-jvm`: the file's views or relationships refer to model
    content that only `!script`, `!plugin` or `!components` would
    create, which the parity table marks never.
  - `needs-env`: the file needs an environment variable. Rust 2024 makes
    `std::env::set_var` `unsafe`, so a test cannot set one. This reason
    is tied to the `${NAME}` environment lookup (dsl-breadth).

  The test `p2_501_manifest_covers_every_dsl` fails on any `.dsl` with
  no line, any line naming a missing file, any duplicate line, and any
  reason outside the list.
- **P2-502** `tests/data/structurizr/` holds the Apache-2.0 `LICENSE`
  text of the source repository. Its `SOURCE` file names the repository
  URL, the commit and both source paths, and says the files are
  unmodified. The test is `p2_502_corpus_licence_present`.
- **P2-28** (kept, made precise) The `#[ignore]`d test
  `p2_28_house_style_report` is run by hand. It builds every MANIFEST
  root and prints one line per root, giving the sorted codes of every
  lint error rule the file breaks over all its built views plus the
  file-level L22 findings (`AllViews::coverage`). It then prints three
  totals that sum to the number of roots:
  - files with no lint error
  - files whose lint errors are all house-style rules
  - files with any other lint error

  The house-style rules are L4 (crossings above `maxCrossings`) and L15
  (C1 line shows a technology), the two the SRS names. `TODO(srs)`:
  whether other c4rizer-only error rules (for example L8, L9, L10, L12,
  L13, L18, L21, L22) also count as house style.
- **P2-503** The same report prints, for each lint error rule, the
  number of root files that break it at least once.
  `p2_503_per_rule_file_counts` checks this count on hand-made findings:
  one file breaking a rule twice counts once.
- **P2-15** (kept) `c4rizer import-layout workspace.json FILE.dsl`
  writes an `at` line, snapped to the 10px grid, for every element
  position stored in the Structurizr `workspace.json` that can be
  matched (P2-506) to a box of a view whose key is in both files. It
  changes nothing else in `FILE.dsl` beyond P2-509's exception. P2-504
  to P2-511 give the details. The old coordinate-mapping `TODO(srs)` is
  now P2-508.
- **P2-504** `c4rizer import-layout JSON FILE` rewrites FILE in place
  and exits 0 when the run completes, even when it skipped items. It
  exits 2, with FILE left byte-identical, in these cases:
  - a usage error, with the usage text on stderr
  - a file that cannot be read
  - a DSL parse error
  - JSON that is not valid RFC 8259
  - JSON with no top-level `views` object (not a Structurizr workspace)
  - an `edit::apply` error

  A JSON syntax error is reported as `path:line:column: <what> — <what
  to change>` (SRS 5.1).
- **P2-505** Views are matched by key. A view in the JSON arrays
  `systemLandscapeViews`, `systemContextViews`, `containerViews`,
  `componentViews` or `customViews` is paired with the c4rizer view
  whose `ViewSpec::key` equals its `key`, compared exactly. A JSON view
  with no c4rizer view of that key is reported as skipped. A c4rizer
  view with no JSON view is left untouched. When a match is on a key
  c4rizer generated (`ViewSpec::declared_key` is `None`), a warning
  names the view, because the reference recommends explicit keys when
  layout is kept and says generated ones are not stable.
- **P2-506** Within a matched view, each box that `view::plan` draws is
  paired with the first JSON element view whose element matches, trying
  the steps of the reference's `DefaultLayoutMergeStrategy` in order:
  1. same canonical name (element type plus the names of its ancestors
     and itself)
  2. same element type and same name
  3. same element type and same non-empty description

  The reference's fourth step, matching by Structurizr element id, does
  not apply, because c4rizer elements carry no Structurizr ids. A JSON
  element view whose `id` is not in the JSON model is skipped with a
  notice.
- **P2-507** An element that the JSON view lists but the c4rizer view
  does not draw is not imported, because the reference copies layout
  only into elements of the destination view. A drawn box with no JSON
  match keeps its current `at` line, or still has none, and gets one
  warning naming the view and the element.
- **P2-508** JSON element positions `x` and `y` are the box's top-left
  corner (reference: `SVGReader` sets the page bound to `x + style
  width`). The `at` centre is `(x + w/2, y + h/2)`, each coordinate
  rounded to the nearest multiple of 10. `w` and `h` are the `width` and
  `height` of the element's resolved style in the JSON's
  `views.configuration.styles.elements`: the element's comma-separated
  `tags` are taken in order and a later tag wins, as in DSL-15.
  `TODO(srs)`: the size to use when no style sets width or height. It is
  Structurizr's renderer default, remembered as 450x300 but not in the
  fetched sources. `TODO(srs)`: whether positions are scaled to
  c4rizer's smaller box sizes (CORE-2); the reference has no such step.
- **P2-509** Positions are written by one `edit::apply` call that holds
  one `Edit::Move` per matched box. An existing `at` line is
  overwritten, because the reference copies layout over the destination.
  A missing one is added per DSL-9. With `!include`, the lines go to the
  root file (P2-5). Every byte outside the written `at` lines is
  unchanged, with one exception: DSL-9 splits a one-line view body `{
  ... }` onto lines before adding the first `at` line. The test checks
  this with a line diff of FILE before and after. FILE is not rewritten
  when there is no edit.
- **P2-510** Layout data that c4rizer cannot hold is not imported, and
  the report names each kind once per view: relationship `vertices`
  (phase 3, P3-4), relationship `routing` and `position` (P2-12
  `TODO(srs)`), and view `paperSize` and `dimensions`. JSON
  `dynamicViews`, `deploymentViews` and `imageViews` are reported as
  skipped, not drawable yet. JSON `filteredViews` hold no positions and
  are ignored, because a c4rizer filtered view takes its base view's
  `at` lines (P2-9). The JSON's `views.configuration.themes` are named
  once and not fetched (P2-13).
- **P2-511** A matched box whose element has no identifier in FILE
  (`Element::key` is `None`, which phase 2 allows) gets no `at` line,
  because an `at` line names an identifier, and the report gives one
  warning naming the view and the element's name. `TODO(srs)`: whether
  import-layout should offer to add an identifier (a text edit, not
  element creation) so that these boxes can keep their layout.

cli-breadth

Sources: https://docs.structurizr.com/commands, /validate, /inspect,
/list and /workspaces/inspections, fetched 2026-11-15.
/export/png-and-svg and /dsl/language were not re-read, because this row
adds no DSL grammar and Structurizr has no SVG text option. The
inspections page was available only as a paraphrase: the inspection keys
and option texts below are exact, but the default-severity sentence is
not quoted word for word. Umbrella ids are kept: **P2-22** is split into
P2-600..P2-650 and **P2-25** into P2-651..P2-665. An umbrella counts as
met when every child is met. Two SRS edits this row needs are open
questions in `docs/deferred.md`: CLI-2 scoped to `render` and `lint` (as
written it would give `inspect` exit 1 on lint errors, against P2-642),
and EXP-1 scoped to `--svg-text name` (its "no `@font-face`" clause
contradicts P2-25 by design).

- **P2-22** (kept, umbrella) The CLI adds `validate`, `inspect`, `list`,
  `version` and `help` with the meaning of the Structurizr commands of
  the same name; met when P2-600..P2-650 are met (no test of its own).
- **P2-25** (kept, umbrella) SVG export (CLI `render` and the editor's
  export) offers an option to embed the font or outline the text instead
  of naming it (EXP-1); met when P2-651..P2-665 are met (no test of its
  own).

**The workspace argument**

- **P2-600** `validate`, `inspect` and `list` each take the diagram file
  as `-workspace FILE`, the spelling of the parity table and
  Structurizr. TODO(srs): should a bare positional FILE also work, as it
  does for `render` and `lint`?
- **P2-601** `-workspace -` reads the diagram from stdin. Errors name it
  `<stdin>`, the same convention as `render` and `lint`.
- **P2-602** `validate`, `inspect` or `list` exits 2 and prints a usage
  error on stderr in each of these cases: no `-workspace`, `-workspace`
  with no value, `-workspace` given twice, or an argument the subcommand
  does not take.
- **P2-603** A `-workspace` value that starts with `http://` or
  `https://` (any case) is a usage error: nothing is fetched (network is
  the phase-3 row). A JSON workspace file fails as a parse error (JSON
  input is phase 3). The workspace may also be given as `-w`,
  `--workspace` or one bare FILE. Phase 3 amends this: a URL needs
  `--network` (P3-603), and a JSON file is read per P3-416.

**validate** (Structurizr: "Validates a workspace.")

- **P2-604** `c4rizer validate -workspace FILE` exits 0 when FILE parses
  into a model with no error. TODO(srs): the text of the success line,
  if there is one.
- **P2-605** `validate` writes no file and runs no placement, routing or
  lint, so lint findings never change its exit code.
- **P2-606** On a parse or I/O error, `validate` prints `c4rizer:
  <path>:<line>:<column>: <what> — <what to change>` on stderr (SRS 5.1)
  and exits 2.
- **P2-607** `validate` exits 2, with a message naming the element that
  breaks the rule, when the workspace breaks its own `configuration
  scope`. This needs P2-18. TODO(srs): the exact scope rules for
  `landscape` and `softwaresystem`. They come from memory of the
  Structurizr scope validation and were not re-read.

**list** (Structurizr: "Lists the elements in a workspace.")

- **P2-608** `c4rizer list -workspace FILE` prints every element of the
  model to stdout exactly once, in declaration order (`Model::elements`,
  so parents come before children), and exits 0. TODO(srs): the line
  format, which the reference does not show.
- **P2-609** `list` writes no file and runs no placement, routing or
  lint.
- **P2-610** On a parse or I/O error, `list` prints the error on stderr
  per SRS 5.1 and exits 2.

**inspect** (Structurizr: "Runs the inspections on a workspace.")

- **P2-611** `c4rizer inspect -workspace FILE` prints one line to stdout
  for each inspection violation it shows. TODO(srs): the exact line
  format and any summary line, which the reference does not describe.
- **P2-612** Each violation line gives the severity, the inspection key
  and the subject: an element id, a relationship as `<source id> ->
  <destination id>`, a view key, or nothing for a workspace-level
  violation.
- **P2-613** `inspect` writes no file and runs no placement, routing or
  lint, so lint findings never change its exit code.
- **P2-614** On a parse or I/O error, `inspect` prints the error on
  stderr per SRS 5.1 and exits 255, so it can be told apart from a
  violation count (P2-642).
- **P2-615** `model.empty`: a model with no elements gives exactly one
  violation.
- **P2-616** `model.person.description`: each person with no description
  gives one violation naming it.
- **P2-617** `model.softwaresystem.description`: each software system
  with no description gives one violation naming it.
- **P2-618** `model.container.description`: each container with no
  description gives one violation naming it.
- **P2-619** `model.container.technology`: each container with no
  technology gives one violation naming it.
- **P2-620** `model.component.description`: each component with no
  description gives one violation naming it.
- **P2-621** `model.component.technology`: each component with no
  technology gives one violation naming it.
- **P2-622** `model.element.disconnected`: each element that is the
  source or destination of no relationship gives one violation naming
  it. TODO(srs): do relationships of its children count, and do implied
  relationships count (P2-7)?
- **P2-623** `model.element.noview`: each element that `Model::included`
  returns for no declared view gives one violation naming it. Default
  views that the editor offers (EDIT-15) and has not written to the file
  do not count.
- **P2-624** `model.relationship.description`: each relationship with no
  description gives one violation naming its source and destination.
- **P2-625** `model.relationship.technology`: each relationship with no
  technology gives one violation naming its source and destination.
- **P2-626** `views.empty`: a workspace with no declared view gives
  exactly one violation.
- **P2-627** `views.view.empty`: each declared view that includes no
  element gives one violation naming its key.
- **P2-628** `model.softwaresystem.documentation`: each software system
  that has containers but no attached documentation (`!docs`) gives one
  violation naming it. This needs the dsl-breadth `!docs` requirement.
- **P2-629** `model.softwaresystem.decisions`: each software system that
  has containers but no attached decisions (`!adrs`) gives one violation
  naming it. This needs the dsl-breadth `!adrs` requirement.
- **P2-630** `views.styles.element.metadata`: each element style that
  sets `metadata false` gives one violation naming its tag. This needs
  P2-10.
- **P2-631** TODO(srs): the reference names five inspections without
  defining their triggers: `workspace.scope` (depends on `configuration
  scope`, P2-18), `workspace.tooling`, `views.view.key`,
  `views.view.layout` and `documentation.embeddedview`. Each one gets
  its own id once its trigger is settled.
- **P2-632** The deployment inspections wait for the phase-3 deployment
  model (P3-1) and are not built in this row:
  `model.deploymentnode.description`, `model.deploymentnode.technology`,
  `model.deploymentnode.empty`, `model.infrastructurenode.description`
  and `model.infrastructurenode.technology`.
- **P2-633** Every violation has severity `error` unless a property from
  P2-634..P2-639 sets another one. (Reference: `error` is the default
  severity of all inspections.)
- **P2-634** A workspace property `structurizr.inspection.<key>` with
  value `error`, `warning`, `info` or `ignore` sets that severity on
  every violation with that inspection key. This needs P2-18
  `properties`.
- **P2-635** The wildcard keys set the severity of every inspection
  under their prefix: `structurizr.inspection.*`,
  `structurizr.inspection.model.*` and
  `structurizr.inspection.model.component.*`. TODO(srs): which key wins
  when an exact key and a wildcard both match.
- **P2-636** A `structurizr.inspection.<key>` property on an element, a
  relationship or a view sets the severity of that subject's violations
  with that key. This needs P2-18.
- **P2-637** A `structurizr.inspection.<key>` property on an element
  also sets the severity of the violations with that key on every
  element beneath it. For example, a property on a container covers its
  components.
- **P2-638** Model-level properties set the severity of model
  inspections, and view-set or workspace properties set the severity of
  view inspections. TODO(srs): which level wins when several set a
  severity.
- **P2-639**
  `structurizr.inspection.model.relationship[<src>-><dst>].technology`
  sets the severity only for relationships whose source and destination
  elements are of those types. The types are `person`, `softwaresystem`,
  `container`, `component`, `deploymentnode`, `infrastructurenode`,
  `softwaresysteminstance` and `containerinstance`. The plain key
  `structurizr.inspection.model.relationship.technology` covers all
  relationships.
- **P2-640** `-severity LIST` (comma-separated values from `error`,
  `warning`, `info` and `ignore`; default `error,warning,info,ignore`)
  makes `inspect` show, and count, only violations with a listed
  severity.
- **P2-641** `-severity` with an unknown value, with no value, given
  twice, or on a subcommand other than `inspect` prints a usage error on
  stderr and exits 2.
- **P2-642** `inspect` exits with the number of error-severity
  violations shown, capped at 254, so 0 means no error was shown. A
  parse, I/O or usage error of `inspect` exits 255; every other command
  keeps exit 2 for errors.
- **P2-643** `-inspector <class>` is refused: c4rizer prints one line on
  stderr saying that custom Java inspectors need the JVM and are not
  supported (parity table: never), and exits 2.
- **P2-644** `inspect` run twice on the same file prints byte-identical
  output (CORE-25).

**version, help, usage**

- **P2-645** `c4rizer version` prints a line containing the workspace
  package version (`CARGO_PKG_VERSION`) on stdout and exits 0.
  TODO(srs): the exact text. Proposal: `c4rizer <version>`.
- **P2-646** `c4rizer version` followed by any argument prints a usage
  error on stderr and exits 2.
- **P2-647** `c4rizer help` prints the usage text on stdout and exits 0.
  TODO(srs): `help <command>` and `-h`/`--help`. The reference only says
  "Displays a help message."
- **P2-648** The usage text names every subcommand (`render`, `lint`,
  `validate`, `inspect`, `list`, `version`, `help`) with each of its
  arguments, including `-workspace`, `-severity` and `--svg-text`.
- **P2-649** Every usage error prints `c4rizer: <error>` and then the
  usage text on stderr, and exits 2.
- **P2-650** The no-subcommand and unknown-subcommand messages name all
  seven subcommands.

**SVG text option (P2-25)**

- **P2-651** `c4rizer render` takes `--svg-text name|embed|outline`.
  TODO(srs): the flag name. Structurizr has no equivalent because it
  exports through a browser.
- **P2-652** Without `--svg-text`, or with `--svg-text name`, every SVG
  `render` writes is byte-identical to the phase-1 EXP-1 output, so the
  CLI-5 goldens do not change.
- **P2-653** `--svg-text` on a subcommand other than `render`, given
  twice, with no value or with an unknown value prints a usage error on
  stderr and exits 2.
- **P2-654** EXP-1 applies in full to `name`. Under `embed`,
  P2-655..P2-657 replace EXP-1's "no `@font-face`" clause. Under
  `outline`, P2-658..P2-659 replace EXP-1's `font-family` clause.
- **P2-655** With `embed`, the SVG carries one `<style>` with one
  `@font-face` for each bundled face the scene's text uses. Each rule
  gives `font-family` equal to `scene::FONT_FAMILY` and a `font-weight`
  that matches the text it serves: `normal` for `scene::FONT_REGULAR`
  and `bold` for `scene::FONT_BOLD`.
- **P2-656** With `embed`, each `@font-face` has `src:
  url(data:font/ttf;base64,…)`, and its decoded bytes equal the bundled
  face exactly. TODO(srs): font subsetting. The full faces are 412 KB
  and 420 KB, about 550 KB each in base64.
- **P2-657** With `embed`, the SVG passes the EXP-1
  no-external-reference scan except for the `data:` URIs inside
  `@font-face` and the icon images P2-409 allows
  (`<image href="data:image/...">`): no script, no other `href` and no
  other `url(...)`.
- **P2-658** With `outline`, the SVG has no `<text>` element, no
  `font-family` and no `@font-face`. Every text line is drawn instead as
  filled paths taken from the bundled faces' glyph outlines, with the
  same fill as the `name` export.
- **P2-659** With `outline`, the SVG passes the EXP-1
  no-external-reference scan with no exception beyond the icon images
  P2-409 allows (`<image href="data:image/...">`, never `xlink:href`).
- **P2-660** An `outline` SVG rasterized with no font loaded matches the
  same view's `name` SVG rasterized with the bundled fonts (EXP-2) at
  the same width, within a pixel tolerance. TODO(srs): the allowed
  difference.
- **P2-661** `--svg-text` changes only the `.svg` files: `--png` output
  is byte-identical whatever its value.
- **P2-662** `--svg-text outline` on a build without outline support
  exits 2 with a message saying how to rebuild, the same way `--png`
  without the `png` feature does.
- **P2-663** The editor's "Export view" and "Export all views" (EDIT-7)
  offer the choices `name`, `embed` and `outline`, with `name` as the
  default.
- **P2-664** An SVG the editor exports with a given choice is
  byte-identical to `c4rizer render --svg-text <choice>` for the same
  view and positions.
- **P2-665** Picking an SVG text choice in the editor writes nothing to
  the diagram file. The `.dsl` holds only model, views and layout.

editor-breadth

P2-23 and P2-24 keep their ids and become umbrellas; the new ids run
from P2-700 to P2-733. Sources fetched 2026-11-15:
https://docs.structurizr.com/server/diagrams/viewer,
/server/diagrams/editor, /server/diagrams/perspectives and
/server/diagrams/customisation (the `/ui/diagrams/...` paths return 404;
the pages moved under `/server/diagrams/`). A point taken from memory
rather than from a fetched page says "from memory" and carries
TODO(srs). Each requirement is one testable sentence; a TODO(srs) or
Note after it is not part of the sentence.

- **P2-23** (kept, umbrella) The Structurizr diagram-editor canvas
  commands work in the editor, which is met when P2-700..P2-712 and
  P2-733 are met (no test of its own).
- **P2-24** (kept, umbrella; its old TODO(srs) "one requirement per
  item" is resolved by the split below) The Structurizr viewer's viewing
  aids work in the editor, which is met when P2-713..P2-732 are met (no
  test of its own).

**Canvas commands (split from P2-23)**

- **P2-700** The "Auto-place view" command places every box of the open
  view with CORE-22 placement as though the view had no `at` lines and
  no previous scene (`place::Keep::default()`, `size` lines kept), then
  writes an `at` line for every box at its new grid-snapped centre as
  one undo step (EDIT-19), writing the default view first when EDIT-15
  needs it, so that running it twice on an unchanged file leaves the
  file byte-identical (CORE-25). Note: Structurizr's version is the
  auto-layout modal ("manually apply the automatic layout algorithm";
  Structurizr uses Dagre). The command takes no options because
  `autoLayout` lines are kept byte for byte and have no effect (DSL-6).
- **P2-701** "Decrease canvas size", "Auto canvas size" and "Increase
  canvas size" set the open view's canvas to the previous size in an
  ordered size list, the smallest size that holds the scene, or the next
  size, and write it to the diagram file (P2-733) as one undo step.
  TODO(srs): (a) the size list (from memory: A6..A0 portrait and
  landscape, Letter, Legal, slide formats; no fetched page lists it);
  (b) the syntax (Structurizr DSL has none and keeps paper size in
  workspace JSON, so c4rizer needs a view extension line, as it has
  `at`); (c) whether SVG/PNG export (EDIT-7, CLI) uses the canvas
  instead of `Scene::bounds` (the parity note says "Paper size for
  export").
- **P2-702** Decrease and Increase canvas size move every box of the
  open view by one shared grid-snapped offset that keeps the drawing
  centred on the new canvas, in the same undo step as the size change,
  unless Alt is held, in which case every box keeps its position. The
  reference says: "By default, the diagram will stay centred when using
  the decrease and increase canvas size buttons", and holding Alt turns
  this off. TODO(srs): the reference names only decrease and increase;
  whether Auto also re-centres.
- **P2-703** "Bring boxes onto the canvas" (the magnet button in
  Structurizr's auto-layout modal, "bring all elements back on to the
  diagram canvas") moves each box of the open view that lies wholly or
  partly outside the canvas to the nearest grid position fully inside
  it, and writes those boxes' `at` lines as one undo step. It writes
  nothing when every box is already inside. TODO(srs): "nearest" is a
  c4rizer choice (the reference says only "back on to"); what it does
  while the canvas is on auto size.
- **P2-704** "Centre" moves the selected boxes, or every box of the open
  view when none is selected, by one shared grid-snapped offset that
  puts the centre of their bounding rectangle at the canvas centre,
  horizontally and vertically, as one undo step. The reference says:
  "Centre diagram or selected elements" and "horizontally and vertically
  centre the selected elements on the diagram canvas". TODO(srs): what
  the canvas centre is while the canvas is on auto size.
- **P2-705** `w`, `h` and `c` set the camera so that the whole scene,
  measured as EDIT-30 "Fit view" measures it (20px margin, never above
  100% zoom), fits the canvas widget's width, its height, or both,
  centred, without changing the file (EDIT-9); on an empty scene the
  camera stays unchanged. Note: `c` ("Zoom to fit content") does exactly
  what EDIT-30 Fit view does.
- **P2-706** A "Fullscreen" command toggles fullscreen for the window
  and keeps the open view, zoom, pan and selection. TODO(srs):
  Structurizr binds `f` ("Enter fullscreen mode"), but phase-1 EDIT-30
  gives the F key to Fit view. The owner picks one: a different key for
  fullscreen, no key, or amending EDIT-30.
- **P2-707** With two or more boxes selected, `l`, `c`, `r`, `t`, `m`,
  `b`, `h` and `v` align left, centre, right, top, middle and bottom,
  and distribute horizontally and vertically; each key gives the same
  file and the same single undo step as the matching EDIT-20 command.
  TODO(srs): which box is the alignment reference. EDIT-20 as built
  aligns to the outermost edge, and the fetched editor page does not say
  what Structurizr does; aligning to the first selected element is from
  memory.
- **P2-708** While the pointer is over a line or its label, Up and Down
  move that line's label one step along the line in Structurizr
  `position` units (0..100, clamped), redraw the label there, and write
  the open view's position for that line to the diagram file as one undo
  step. The reference says: "`Up` and `Down` arrows can be used to move
  the positioning of the label". TODO(srs): which key moves towards the
  source, and the step size; the per-view syntax (the DSL has `position`
  only on relationship styles, and Structurizr keeps per-view positions
  in workspace JSON); how CORE-14/15 label placement honours a position
  (shared with P2-12).
- **P2-709** While the pointer is over a line or its label, `r` cycles
  that line's routing Direct → Orthogonal → Curved → Direct, writes it
  for the open view to the diagram file as one undo step, and redraws
  the line. The reference says `r` "will toggle the routing algorithm
  between direct, orthogonal, and curved". TODO(srs): what each routing
  means under the octilinear router (P2-12); the per-view syntax.
- **P2-710** While the pointer is over a line or its label, `j` toggles
  jumps for that line in the open view, written to the diagram file as
  one undo step; with jumps on, the line is drawn with a jump at each
  crossing, the same in canvas, SVG and PNG (EDIT-12). Note: the jump
  drawing belongs to styles-themes (parity row `jump`). TODO(srs): the
  per-view syntax; the jump shape on a 0/45/90 line.
- **P2-711** `+` and `-` zoom in and out by one `ZOOM_STEP` about the
  canvas widget's centre, clamped to `MIN_ZOOM..=MAX_ZOOM`, without
  changing the file (EDIT-9). Note: the reference gives the keys but not
  the zoom anchor; the widget centre is c4rizer's choice.
- **P2-712** The single-key bindings of this row act only when no text
  field has focus and no in-place edit is open (EDIT-5), and each key
  goes to the first owner that takes it, in this order, while EDIT-16,
  EDIT-19 and EDIT-28 chords keep their meaning (Ctrl+Z, Ctrl+Shift+Z,
  Ctrl+Y, Alt+Left, Backspace, Enter, F2 and Escape-deselect):
  1. An open quick-navigation list (P2-715) takes every key.
  2. A line or label under the pointer takes Up, Down, `r` and `j`.
  3. A selection of two or more boxes takes `l c r t m b h v`.
  4. Otherwise the viewer bindings apply.

  TODO(srs): this order is a proposal. Structurizr has separate editor
  and viewer modes, so its keys clash (`l c r t m b h`, Up/Down), and
  the reference does not settle one combined table.

**Viewing aids (split from P2-24)**

- **P2-713** Up or Right opens the previous view of the view list
  (`State::views`, EDIT-8, ordered per P2-716), and Down or Left opens
  the next, unless P2-712 gives the key elsewhere. The reference reads:
  "`Up arrow` Navigate to the previous diagram", "`Down arrow` … next",
  "`Left arrow` … next", "`Right arrow` … previous". TODO(srs): the
  Left/Right mapping looks inverted but is copied as quoted; whether the
  list stops or wraps at its ends.
- **P2-714** `b` opens the last viewed view, the one shown before the
  current one in this session's history of views shown, which is not the
  scope-derived parent crumb of EDIT-16 (reference: "Go back to the last
  viewed diagram").
- **P2-715** A Space press released with no pointer drag in between
  opens quick navigation, a list of every view of the view list filtered
  by the typed text, in which Enter opens the highlighted view and
  Escape closes the list and changes nothing; Space held during a drag
  pans (EDIT-28) and never opens the list. The reference says only:
  "`Space` open quick navigation". TODO(srs): the matching rule (key or
  title, substring or prefix, case).
- **P2-716** The view-set property `structurizr.sort` orders the view
  list (EDIT-8), and so P2-713: `key` sorts by view key, `created` by
  declaration order ("the order in which views are defined in your
  DSL"), and `type` by view type ("keep all system context views
  together"). Note: the parsing comes from dsl-breadth (P2-18).
  TODO(srs): the order among types, and tie-breaks; with no property
  set, keep EDIT-8's declaration order or take Structurizr's default
  ("first sorted by scope and then by diagram type", whose details are
  not on the fetched page)?
- **P2-717** A file whose views contain a `default` statement opens on
  that view when the per-user state has no remembered view for the file,
  so the open order becomes remembered view, then `default` view, then
  first declared view (extends EDIT-13). Note: the parsing comes from
  landscape-filtered (P2-20). TODO(srs): confirm that a remembered view
  beats `default`; whether EDIT-27's fallback after a reload also
  prefers the `default` view.
- **P2-718** An Alt+click on a box toggles its selection as a
  Shift-click does and never enters the box, so an Alt+double-click
  leaves both the open view and the selection as they were. The
  reference says: "Hold `Alt` or `Shift` while clicking an element … to
  multi-select", and the parity row reads "Multi-select
  (Shift/Alt-click, lasso)". Note: this fills a gap in EDIT-28, whose
  table names only Shift.
- **P2-719** An Alt+double-click on a box whose element has a `url`
  emits an effect that opens that URL in the system browser; on a box
  with no `url`, nothing opens and the status bar says so. The reference
  says: "Double-clicking with the `Alt` key pressed will open the URL".
  TODO(srs): whether only `http`/`https` URLs are opened; the
  reference's double-click also offers the URLs in an element's
  `properties` (name=label, value=url) and its docs and decisions (docs
  and decisions belong to phase-3 `explore`; property URLs have no
  parity row).
- **P2-720** An Alt+double-click on a line or its label opens the `url`
  of the relationship it draws; for a lifted or merged line this is the
  relationship EDIT-5 edits (`PlanEdge::winner`), and with no `url`
  nothing opens and the status bar says so. The reference says:
  "Double-clicking relationships (hold the `Alt` key while in the
  diagram editor)".
- **P2-721** `d` toggles descriptions: while they are off, no element or
  relationship description is drawn on the canvas, and box sizes, box
  positions, lines and label positions are unchanged. The reference
  says: "Toggle element/relationship descriptions on/off". TODO(srs):
  keeping the geometry is a c4rizer choice (P2-730: viewing aids are
  view-only); how the toggle combines with the element style
  `description false` (P2-10).
- **P2-722** `m` toggles metadata: while it is off, each box's type line
  (CORE-2, for example `[Container: Rust]`) and the technology in each
  line label are not drawn, with geometry unchanged as in P2-721. The
  reference says: "Toggle element/relationship metadata on/off".
  TODO(srs): "metadata = type line plus technology" is from memory, not
  from a fetched page; how the toggle combines with the element style
  `metadata false` (P2-10).
- **P2-723** `t` toggles tooltips, and while tooltips are on, hovering a
  box or line shows an overlay with its EDIT-10 details; the initial
  state comes from `structurizr.tooltips` on the view, else on the view
  set ("Viewset or view", `true`/`false`). TODO(srs): the initial state
  when neither property is set (the reference gives no default); tooltip
  content beyond the EDIT-10 fields (properties, perspectives, URL).
- **P2-724** Holding Alt while a tooltip is displayed makes it sticky,
  so it stays shown after the pointer leaves its item. The reference
  says: "hold the `Alt` key while the tooltip is displayed to enable
  sticky tooltips". TODO(srs): what releases a sticky tooltip (`t`,
  Escape, the next hover), and how Escape is shared with EDIT-28
  deselect.
- **P2-725** `g` opens a tag filter that lists the user-defined tags of
  the open view's boxes and lines, meaning their effective tags minus
  the built-ins `Element`, `Person`, `Software System`, `Container`,
  `Component` and `Relationship`; choosing tags marks as filtered out
  every box and line that carries none of them, and clearing the choice
  restores the view. The reference says: "Filter by user-defined tags".
  TODO(srs): whether filtered-out items are faded or hidden; whether
  several chosen tags match any or all; the full built-in tag list once
  groups (P2-4) and phase-3 deployment tags land.
- **P2-726** `p` lists the perspective names found on the open view's
  elements and relationships (P2-18); choosing one shows the view "with
  a given perspective" (reference), and choosing none returns to the
  normal view. TODO(srs): what is drawn. The perspectives page does not
  say. From memory, items without the perspective are faded and the
  others show its value.
- **P2-727** `n` shows or hides the view-list panel (EDIT-8, EDIT-32)
  without changing the open view, zoom, pan or file. The reference says:
  "`n` Toggle diagram navigation on/off", and lists the diagrams in "the
  navigation panel". TODO(srs): the parity table has no row for `n`, `i`
  or `l`, which is a defect under section 2. Add one.
- **P2-728** `i` shows the diagram key/legend (reference: "`i` Show
  diagram key/legend"). TODO(srs): c4rizer draws the key inside the
  scene (CORE-1), so "show" needs a testable meaning: pan to it, an
  overlay, or toggle it.
- **P2-729** When P2-712 does not give `l` to a selection, `l` opens the
  diagram navigator, which shows the views that sit above, next to and
  below the open view (reference: "The diagram navigator provides a way
  to see which diagrams in the workspace sit “above”, “next to”, and
  “below”"). TODO(srs): the parity table maps the navigator to EDIT-8
  and the breadcrumb (EDIT-16); the form of the navigator.
- **P2-730** None of P2-705, P2-706, P2-711 or P2-713..P2-729 writes the
  diagram file or adds an undo step.
- **P2-731** After an external reload (EDIT-1), the description,
  metadata and tooltip toggles, the tag filter and the chosen
  perspective stay as they were, as view, zoom and pan do, except that a
  chosen tag or perspective no longer present in the open view is
  dropped. TODO(srs): whether these are also remembered per file in the
  per-user state (EDIT-13); whether SVG/PNG export applies them.
- **P2-732** The only differences between the canvas and the SVG export
  of the same view, beyond EDIT-12's selection and hover marks, are the
  tooltip overlay, the quick-navigation list, the tag-filter and
  perspective marks, and the texts hidden by P2-721/P2-722 (this amends
  EDIT-12).
- **P2-733** Every write made by this row goes to the open `.dsl` file
  as lines of the view it was made in (`at` lines, and the view
  extension lines of P2-701 and P2-708..P2-710), never to a workspace
  JSON, a side file or the per-user state (EDIT-13), and no command of
  this row creates or deletes an element or relationship (EDIT-21).
- **P2-734** The breadcrumb names each crumb's C4 level with its
  element, e.g. `Context: Atomanja › Container: Atomanja server ›
  Component: web`, so two crumbs with the same element name are told
  apart and the current level is always visible.
- **P2-735** Resize handles of a selected box never cover a line end or
  arrowhead: a handle that would is drawn outside the box edge.
- **P2-736** The style panel's shape choice is a single drop-down, never
  wrapped over lines.
- **P2-737** The view list shows each view on one line (title truncated
  with an ellipsis, then the key); the full title is a tooltip.
- **P2-738** A fitted view keeps box and label text at least 9px high on
  screen: when fitting would go smaller, the fit stops at that zoom and
  the view can be panned. `TODO(srs)`: confirm 9px after measuring.
- **P2-739** In the details panel, the name, description and technology
  fields show an edit affordance (a pencil mark and a hover outline);
  a double click or Enter edits them in place (EDIT-5).

### 4.7 Phase 3

Requirements grouped by ROADMAP phase 3 row, one testable sentence each
(rows' step 2, approver-signed 2026-11-18; source
`docs/plans/phase3-srs-proposal.md`). The parity table (section 2) lists
every feature they cover. Ids P3-1..P3-9 are the original stubs, kept as
umbrellas; new ids are allocated in one block per row so rows never
collide: deployment-view P3-100.., dynamic-view P3-200.., vertices
P3-300.., exporters P3-400.., explore P3-500.., network P3-600..
*(probed)* marks behaviour checked against Structurizr 6.2.4 (CLI
v2026.10.26) in scratch files on 2026-11-18.

Shared terms:

- **Fixture**: a file under `tests/data/phase3/`.
- **Reference output**: a file that Structurizr 6.2.4 wrote once from a
  fixture and that is checked in under `tests/data/phase3/structurizr/`.
  Tests never run Java or touch the network.

Build order (approved): wave A = network, deployment-view, dynamic-view
and the explore tables (P3-500..P3-507); wave B = vertices, exporters
(JSON in first) and explore graph, tree and docs; wave C = the exporters'
text formats and merge/generate. Animation (P3-222..P3-226) lands last
in dynamic-view.

deployment-view

Authority: SRS 4.7 P3-1, P3-2; parity rows `deploymentEnvironment`,
`deploymentGroup`, `deploymentNode`, `infrastructureNode`,
`softwareSystemInstance`, `containerInstance`, `instanceOf`,
`instances`, `-/>`, view type `deployment`; P2-120, P2-179, P2-2,
P2-306, P2-632. Fixture `tests/data/phase3/deployment.dsl`.

**Model**

- **P3-1** `deploymentEnvironment`, `deploymentNode`,
  `infrastructureNode`, `softwareSystemInstance`, `containerInstance`
  and `instanceOf` build a deployment model in which every node,
  infrastructure node and instance knows its parent node and its
  environment. Deployment nodes, infrastructure nodes and instances are
  elements of the model, of four new element kinds, so styles,
  expressions, `at` lines, edits and the details panel address them as
  they address any element; a static view's `include *` still selects
  none of them (P2-120). Split into P3-100..P3-109.
- **P3-100** Every `deploymentEnvironment` gives one environment in the
  model, holding its name and, when one is written, its identifier, in
  declaration order. Two environments with the same name are a
  `DuplicateName` error naming both lines.
- **P3-101** A `deploymentNode` gives one deployment node holding its
  name, description, technology, tags, `instances` value, parent node
  (none at the top level) and environment. A nested `deploymentNode` has
  the enclosing node as parent.
- **P3-102** `instances` takes a positive integer or a range `m..n` with
  `m <= n`. The value is kept as written and defaults to `1`. Any other
  value is an error naming it.
- **P3-103** An `infrastructureNode` gives one infrastructure node with
  its name, description, technology, tags and parent node.
- **P3-104** `softwareSystemInstance <id>`, `containerInstance <id>` and
  `instanceOf <id>` each give one instance of the named element. It
  holds its parent node, its deployment groups (default: none) and an
  instance number. Instances of one element within one environment are
  numbered 1, 2, ... in declaration order.
- **P3-105** An instance carries the tag `Software System Instance` or
  `Container Instance` and its own written tags. For styling it also
  takes the tags of the element it is an instance of, applied before its
  own (Structurizr `getAllTagsForElement`).
- **P3-106** Copied relationships. For every model relationship `a -> b`
  whose two ends each have at least one instance in the same environment,
  each pair (instance of `a`, instance of `b`) gets one copied
  relationship with the same description, technology and tags. A pair
  gets one only when its two instances share a deployment group; an
  instance with no group counts as being in a shared default group.
  On the fixture this gives `A_1 -> B_1` and `A_2 -> B_2` but not
  `A_1 -> B_2` *(probed)*.
- **P3-107** A relationship written in a deployment block between a
  node or infrastructure node and any deployment element is kept. A
  relationship written between two software system or container
  instances is an error naming both ends ("not permitted", as
  Structurizr).
- **P3-108** `a -/> b` in a deployment block removes the copied
  relationship between instances `a` and `b` (both must be instances of
  that environment). When there is no such copied relationship, it is an
  error naming both ends. Structurizr 6.2.4 refuses `-/>` in every block
  probed; c4rizer keeps accepting it with this meaning, a superset that
  breaks no Structurizr file, recorded as "differs (accepted)" in the
  parity table.
- **P3-109** Under `!identifiers hierarchical`, a deployment id is
  addressed by its full dotted path from the environment, as Structurizr
  6.2.4 does (`live.aws.ec2.webi`), and a bare id inside the enclosing
  block resolves as P2-142 does. This replaces the flat-id `DECISION`
  in deploy.rs.

**Drawing**

- **P3-2** A `deployment <system|*> <environment> ["key"]
  ["description"]` view draws deployment nodes as nested boundaries
  with their instances and infrastructure nodes inside. Each drawn
  instance gets its own view-local key. `ViewKind::Deployment` becomes
  drawable and the view leaves `AllViews::unsupported`. Split into
  P3-110..P3-131.
- **P3-110** In a view scoped to system `S`, `include *` selects every
  container instance of `S`, every software system instance and every
  infrastructure node of the environment. It selects no person, and no
  container instance of another system *(probed)*.
- **P3-111** In a `deployment *` view, `include *` selects every
  container instance, every infrastructure node, and every software
  system instance except those of a system that has a container instance
  in the view *(probed)*.
- **P3-112** `include <id>` / `exclude <id>` take deployment node,
  infrastructure node and instance ids. Including a node includes every
  instance and infrastructure node under it. Element expressions (P2-20)
  select deployment elements by `element.type==DeploymentNode|
  InfrastructureNode|SoftwareSystemInstance|ContainerInstance`, tag,
  technology and properties.
- **P3-113** A deployment node is drawn only when it contains, at any
  depth, a drawn instance or infrastructure node. On the fixture, node
  `Empty` is not drawn *(probed)*.
- **P3-114** A drawn deployment node is a boundary with a solid border
  around everything drawn under it. Its label at the top left is the
  name, with `[Deployment Node: <technology>]` underneath
  (`[Deployment Node]` when there is no technology). A nested node's
  boundary lies wholly inside its parent's boundary with at least 20 px
  between them.
- **P3-115** A node whose `instances` is not `1` shows `x<value>`
  (`x2`, `x1..3`) inside its boundary at the bottom-right corner, in the
  node's text colour (Structurizr renderer `structurizrInstanceCount`).
- **P3-116** An instance box draws its element's name, description and
  technology, with the type line `[Container: <tech>]` or
  `[Software System]`, styled by P3-105's tags. An infrastructure node
  box has the type line `[Infrastructure Node: <tech>]`.
- **P3-117** Every drawn instance, infrastructure node and node has a
  view-local key. It is the written identifier when there is one,
  otherwise `<element key>_<instance number>` (the PlantUML export's
  form, P3-104). Two instances of one element in one view are two boxes
  with two keys.
- **P3-118** The view draws every copied relationship (P3-106) and
  every written deployment relationship whose two ends are drawn, lifted
  to the nearest drawn ancestor by CORE-5. A model relationship between
  elements is never drawn directly on a deployment view.
- **P3-119** The default title is the `title` line, else the
  description. The subtitle is the c4rizer `subtitle` line. A view with
  neither title nor description gets L8 as in phase 1 (CORE-1).
- **P3-120** A deployment view with no key gets `Deployment-NNN`
  (P2-124, already built). It needs no change and is listed here so
  that row's tests cover it.

**Placement and routing**

- **P3-121** Auto-placement treats each node boundary as a unit: the
  children of a node, nested nodes included, are placed as a block
  inside it. Sibling boundaries do not overlap and keep at least 60 px
  between them (L6 applied to boundaries). With no `at` line, two runs
  give equal centres (CORE-25).
- **P3-122** `at` and `size` lines take instance and infrastructure
  node ids only. A node's boundary is always computed from its children
  and is never written. An `at` or `size` line naming a deployment node
  is an error naming the id and saying that node boundaries follow their
  contents.
- **P3-123** A line crosses a node boundary only to reach an end inside
  it, and crosses each boundary on its path at most once. A line between
  two boxes in the same node stays inside that node's boundary.
- **P3-124** Layout targets on the fixture's view `live`: no L1, no L3,
  no L24, crossings at most `maxCrossings` (L4), and building the view
  takes under 1000 ms in release (P2-111 budget, `perf_p3_124`).

**Lint and inspections**

- **P3-125** On a deployment view, lint runs L1..L14 and L23..L26, and
  none of L15..L21. L26 reads "a box drawn outside the boundary of its
  own node, or inside the boundary of a node that is not one of its
  ancestors".
- **P3-126** New rule L27 (error): a line crosses a node boundary that
  contains neither of its ends (P3-123).
- **P3-127** L10 (no technology) on a container instance reads the
  technology of the container it is an instance of.
- **P3-128** The inspections of P2-632 are built:
  `model.deploymentnode.description`, `model.deploymentnode.technology`,
  `model.deploymentnode.empty` (a node with no child node, no
  infrastructure node and no instance),
  `model.infrastructurenode.description` and
  `model.infrastructurenode.technology`. Each gives one violation naming
  the node.
- **P3-129** For `views.view.empty` (P2-627), a deployment view counts
  the elements its `include *` selects (P3-110/P3-111). This closes the
  open deferred row "P2-623/P2-627".

**Other views and the editor**

- **P3-130** A `filtered` view whose base is a deployment view draws
  that view with the tag filter of P2-9. This closes the deferred P2-306
  row.
- **P3-131** In the editor, dragging an instance or infrastructure node
  writes its `at` line (DSL-9). Its node boundaries are recomputed and
  re-routed on the next build (EDIT-25). A click on a node boundary
  selects it, and the details panel shows name, technology, `instances`
  and environment (EDIT-10). A node is never enterable (EDIT-15).

dynamic-view

Authority: SRS 4.7 P3-3, P3-7; parity rows view type `dynamic`, dynamic
`[order:]` steps, parallel `{ }` blocks, `animation`, PNG/SVG
`-animation true`, UI "Animation step forward/back, start/stop"; P2-121.
Fixture `tests/data/phase3/dynamic.dsl`, holding the views `seq`, `comp`
and `star`.

**Steps**

- **P3-3** A `dynamic <scope|*> ["key"] ["description"]` view draws its
  steps in order, each label prefixed with its sequence number. The same
  relationship listed twice is drawn as two lines with two view-local
  keys. Split into P3-200..P3-221.
- **P3-200** Each step is stored in the view in file order, holding its
  source, destination, the model relationship it stands for, its
  written description and technology (if any), its sequence number and
  its span. `ViewKind::Dynamic` becomes drawable.
- **P3-201** A step `a -> b` stands for the first-declared relationship
  from `a` to `b`, which may be implied (P2-7). When there is none, it
  is an error naming both ends: "a relationship between A and B does not
  exist in the model" *(probed)*.
- **P3-202** A step `<relationship id> [desc]` stands for that
  relationship. An id naming no relationship is a DSL-5 error.
- **P3-203** A step with no description takes the relationship's
  description. A step with no technology takes the relationship's
  technology *(probed: `u -> a` drew "1: Uses")*.
- **P3-204** In a view scoped to software system `S`, a step end may be
  a person, a software system other than `S`, or a container of any
  system. A component end is an error: "components can't be added to a
  dynamic view when the scope is a software system" *(probed)*.
- **P3-205** In a view scoped to container `C`, a step end may be a
  component of `C`, a container, a person or a software system other
  than the one holding `C`. A component of another container is an
  error naming it.
- **P3-206** In a `dynamic *` view, a step end must be a person or a
  software system. Any other kind is an error: "only people and
  software systems can be added to this dynamic view" *(probed)*.
- **P3-207** A step end equal to the view's scope is an error: "<name>
  is already the scope of this view" *(probed)*.

**Numbering**

- **P3-208** Steps with no written order are numbered `1, 2, 3, ...` in
  file order.
- **P3-209** A written `n:` sets that step's number to `n` as written,
  and the following unwritten steps continue from `n + 1` when `n` is an
  integer.
- **P3-210** Parallel blocks follow Structurizr 6.2.4's
  `ParallelSequenceCounter`: every inner `{ }` of an outer `{ }` starts
  from the number before the outer block. On the fixture view `seq`
  (`u->a; { {a->b} {a->t; a->b} }; b->a; a->t`), the numbers are
  `1, 2, 2, 3, 3, 4` *(probed)*.
- **P3-211** Each drawn line's label is `<number>: <description>`, and
  the technology line follows CORE-14 and the `technology` option
  (renderer `order + ": "`).

**Drawing**

- **P3-212** Every step is one drawn line, even when an earlier step has
  the same two ends. Each step's view-local key is
  `<relationship key or source->destination>#<number>`, unique in the
  view. Lines between the same two boxes run as lanes (CORE-12).
- **P3-213** The boxes drawn are exactly the step ends (no `include`
  line exists in a dynamic view, `DYNAMIC_LINES`). There is no lifting:
  each step is drawn between its own two ends.
- **P3-214** A system-scoped view draws the scope as a dashed boundary
  around the containers of `S` that are drawn. A container-scoped view
  draws the container boundary inside the boundary of its system
  *(probed `comp`)*. `dynamic *` has no boundary.
- **P3-215** Auto-placement reads the step order: of two boxes, the one
  that first appears as a step source is placed in an earlier or equal
  rank (P2-100 applied to steps in order). On the fixture, `seq` has no
  L1, no L3 and no L24.
- **P3-216** Lint on a dynamic view runs L1..L13 and L23..L26, and
  none of L14..L21: L14 (A -> B with B -> A) is a normal
  request/response there, and the C1/C2/C3 kind rules do not apply,
  while the label rules L12 and L13 stay.
- **P3-217** For `views.view.empty` (P2-627), a dynamic view counts its
  steps. A dynamic view with no step gives one violation. This closes
  the deferred P2-623/627 row.

**Editing**

- **P3-218** The details panel of a selected step line shows its
  number, description, technology, its two ends and the relationship it
  stands for (EDIT-10). Enter edits the step's own description in place
  (EDIT-5) and writes only that step's line.
- **P3-219** A "Steps" list beside a dynamic view shows its steps in
  number order. Selecting a row selects that line on the canvas, and the
  other way round.
- **P3-220** In the Steps list, moving a plain step up or down past one
  neighbour in the same block swaps the two step lines in the file,
  keeping each line's bytes and indentation. Only those lines change,
  and the edit is one undo step (EDIT-19).
- **P3-221** A reorder in a view that uses a written `n:` order, or
  across a `{ }` block border, is refused with a status-bar message
  naming the reason, and no file changes: written orders are never
  renumbered automatically.

**Animation (P3-7)**

- **P3-7** A view can be animated step by step. The editor steps
  forward and back and starts and stops it, and PNG/SVG export can
  write one image per step. One animation engine serves every view
  kind; it lives in this row and lands last in it, after deployment-view
  merges (P3-223). Split into P3-222..P3-226.
- **P3-222** A dynamic view's animation steps are its distinct sequence
  numbers in ascending order. Step `k` shows every box touched by a
  line with number `<= k` and those lines. Lines sharing a number appear
  together (renderer `startAnimation`).
- **P3-223** A static or deployment view with an `animation { }` block
  has one step per line of the block. Step `k` shows the elements named
  in lines `1..k` and the lines whose two ends are both shown. Naming an
  element not in the view is an error naming it. A line that names only
  deployment nodes is an error *(probed)*. The P2-3 notice for
  `animation` is removed.
- **P3-224** In the editor, Start shows step 1, and the keys
  Right/Left (and buttons) go forward and back. Stop or Escape shows the
  whole view. Animation changes no file.
- **P3-225** `c4rizer render --animation FILE -o DIR` writes, for each
  animated view, `<key>-<k>.svg` (and `.png` with `--png`) for `k = 1..`
  steps, plus the usual `<key>.svg`. A view with no animation writes
  only `<key>.svg`.
- **P3-226** Every step image of P3-225 has the full view's canvas
  size and box positions, so the frames overlay exactly.

vertices

Authority: SRS 4.7 P3-4, P3-5; SRS 2 "hand-added line vertices as an
override on top of the router (the router stays the default)"; parity
rows UI "Add, move, remove line vertices" and "Remove all relationship
vertices". Structurizr keeps vertices in workspace JSON
(`relationshipView.vertices`, `Vertex{x,y}`), has no DSL form, and edits
them with the jointjs `Vertices` tool. Fixture
`tests/data/phase3/vertices.dsl` (a copy of `example.dsl` view C2 with
`via` lines).

- **P3-4** A line can carry hand-added vertices, written to the file as
  one `via` line per drawn line in the view (not one line per point).
  The line passes through them, still at 0°, 45° or 90° and on the grid,
  and the router routes only the parts between them. Split into
  P3-300..P3-318.
- **P3-300** In a view block, `via <src> -> <dst> <x1> <y1> [<x2> <y2>
  ...]` gives the drawn line from `<src>` to `<dst>` its vertices in
  order. Coordinates are px, multiples of 10. Like `at` and `size`, one
  `via` line holds every point of its line, so an edit rewrites a single
  line.
- **P3-301** In a dynamic view, `via <n>: <src> -> <dst> ...` names the
  step with number `n`, and the plain form names the step with the
  lowest number between those ends.
- **P3-302** A `via` line naming a line that the view does not draw is
  an error naming the line. An odd number of coordinates, or a
  coordinate that is not a multiple of 10, is an error naming the value.
  Two `via` lines for one line are a `DuplicateVia` error naming both.
- **P3-303** A line with vertices `v1..vk` is routed as `k + 1` legs:
  start port to `v1`, `v1` to `v2`, ..., `vk` to end port. Each leg is
  routed by the phase-1 router under CORE-6..CORE-13, so every segment
  is at 0°, 45° or 90° and on the 10 px grid.
- **P3-304** A routed line with vertices passes through every vertex
  point exactly, in order.
- **P3-305** A vertex leg may bend wherever the router chooses. The
  vertex point itself may be a bend, and only there may the turn exceed
  45° (the user asked for that point).
- **P3-306** Start and end ports of a line with vertices are chosen on
  the box side facing `v1` and `vk` respectively (CORE-9 port rules,
  applied to the vertex instead of the other box).
- **P3-307** Lines without vertices are routed after the lines with
  vertices, and treat the vertex legs as fixed obstacles for crossings
  and lanes (CORE-8, CORE-12).
- **P3-5** Removing a line's vertices returns it to the router. With no
  `via` line anywhere, the scene of each `example/` view is
  byte-identical to the phase-2 scene (goldens unchanged).
- **P3-308** A line with vertices never joins a trunk (`bus`, CORE-8)
  and is never a trunk for another line's join.
- **P3-309** New lint rule L28 (warning): a vertex lies inside a box
  rectangle, or two consecutive vertices of one line are the same point.
  L1..L4 and L23 count lines with vertices like any other line.

**Editor**

- **P3-310** With a line selected, a left click on it adds a vertex at
  the nearest grid point and writes the line's `via` line (DSL-9
  placement rules: rewrite the existing `via` line, else add it after
  the view's last `at` line).
- **P3-311** Each vertex of the selected line shows a handle. Dragging
  a handle moves the vertex on the 10 px grid, re-routes the legs off the
  UI thread (EDIT-25), and on release rewrites only that `via` line.
- **P3-312** A double click on a handle removes that vertex. Removing
  the last vertex deletes the `via` line.
- **P3-313** "Remove all vertices" (parity row) deletes every `via` line
  of the open view as one undo step (EDIT-19). The canvas then equals
  the routed view with no vertices.
- **P3-314** Moving one box leaves every vertex where it is, as
  Structurizr does. When a multi-select drag moves both ends of a line by
  the same delta, that line's vertices move by the delta too, all in one
  write.
- **P3-315** Every vertex edit is atomic and checked against the file
  on disk (EDIT-22, EDIT-24), and is one undo step.
- **P3-316** SVG and PNG export draw lines with vertices exactly as the
  canvas does (EDIT-12, one scene).
- **P3-317** `c4rizer import-layout` (compat-fixtures) copies
  `relationshipView.vertices` from a Structurizr JSON as `via` lines,
  each point rounded to the nearest multiple of 10.
- **P3-318** The JSON export (P3-413) writes each line's vertices as
  `relationships[].vertices` of its view.

exporters

Authority: SRS 4.7 P3-6, P3-8; parity rows export `plantuml`,
`plantuml/c4plantuml`, `mermaid`, `websequencediagrams`, `static`,
`json`, `theme`, DOT, `ilograph`, PNG/SVG `-animation`, CLI `export`,
`merge`, `generate system-landscape`, `structurizr.dsl` properties;
P2-603 (JSON input is phase 3); compat-fixtures plan (P3-8 reuses
`import`). Structurizr CLI 6.2.4 `export -f` takes
`plantuml[/structurizr|c4plantuml]|websequencediagrams|mermaid|json|theme|static|fqcn`
*(probed)*; DOT and Ilograph live in `structurizr-export-6.2.4.jar`
(`DOTExporter`, `IlographExporter`) but the CLI does not expose them.
Fixtures: `tests/data/example.dsl`, `tests/data/phase3/deployment.dsl`
and `tests/data/phase3/dynamic.dsl`, with reference outputs under
`tests/data/phase3/structurizr/<format>/`.

**Command**

- **P3-6** Each phase-3 export format is written by the CLI (`export`)
  and by the editor's export, each with a fixture test: `plantuml`,
  `plantuml/c4plantuml`, `mermaid`, `websequencediagrams`, `static`,
  `json` (with the `structurizr.dsl` property), `theme`, `dot` and
  `ilograph`. Split into P3-400..P3-424 and P3-431..P3-432.
- **P3-400** `c4rizer export -f <format> [-o DIR] [-w] FILE` writes the
  format's files into `DIR` (default: FILE's folder, as Structurizr's
  `-output`) and exits 0. Accepted spellings are `-f`, `--format`,
  `-o` and `--output`. `-f plantuml` means `plantuml/structurizr`.
- **P3-401** An unknown format, a missing `-f`, or `-f fqcn` / any Java
  class name is a usage error (exit 2) listing the accepted formats.
- **P3-402** `export -f svg` and `-f png` write the same files as
  `render` (CLI-1, CLI-4), so `export` covers every format.
- **P3-403** Every text export is deterministic: the same file gives
  the same bytes on every run and machine, with `\n` line endings and a
  final newline (CORE-25).
- **P3-404** Per-view formats write one file per drawable view, named
  `structurizr-<key>.<ext>` as Structurizr names them (`.puml`, `.mmd`,
  `.wsd`, `.dot`). PlantUML (`plantuml/structurizr`) also writes
  `structurizr-<key>-key.puml`.
- **P3-405** Views that a format cannot express are skipped with one
  stdout line `== <key>: skipped (<format> does not support <kind>
  views)` and do not change the exit code. This covers image views for
  every text format, and every non-dynamic view for
  `websequencediagrams`.

**Diagram formats**

- **P3-406** `plantuml/structurizr` output for each fixture view equals
  the reference output byte for byte. The same byte-equality rule holds
  for `c4plantuml`, `mermaid`, `websequencediagrams`, `dot` and
  `ilograph` (P3-407..P3-410, P3-412); `json` alone is compared
  semantically (P3-413, P3-416). A byte difference is accepted only
  where Structurizr's output is plainly a bug, and each accepted one is
  named here with its view: the duplicated `S.B` rectangle in the
  PlantUML of view `comp`.
- **P3-407** `plantuml/c4plantuml` output equals the reference output
  byte for byte (P3-406).
- **P3-408** `mermaid` output equals the reference output byte for byte
  (P3-406), including Structurizr's numeric element ids. Those are
  allocated in model creation order as Structurizr allocates them.
- **P3-409** `websequencediagrams` writes one file per dynamic view: a
  `title` line, one `actor` (person) or `participant` line per
  participant in first-use order, and one `<src>-><dst>: <description>`
  line per step in number order. On the fixture it equals the reference
  output byte for byte.
- **P3-410** `dot` writes one Graphviz file per view: a `digraph` with
  one `subgraph cluster_<id>` per boundary (scope, group, deployment
  node), one node per box labelled with name and type line, and one edge
  per drawn line labelled with its description. It equals, byte for
  byte, the output of `structurizr-export` 6.2.4 `DOTExporter` on the
  fixtures, generated once into the reference folder.
- **P3-411** Element positions: no text format carries c4rizer
  coordinates by default, as Structurizr's output has none. The
  exceptions are `json` (P3-413) and `dot` with `--positions`, where
  each node gets `pos="<x>,<y>!"` from the scene (Graphviz honours
  `pos`).
- **P3-412** `ilograph` writes one `workspace.idl.yaml` for the whole
  workspace (resources nested as the model, one perspective per static
  view, one sequence perspective per dynamic view). It equals, byte for
  byte, `structurizr-export` 6.2.4 `IlographExporter` output on the
  fixtures.

**JSON out and in**

- **P3-413** `json` writes `<file stem>.json`, a Structurizr workspace
  JSON. Read back by Structurizr 6.2.4 (`export -w x.json -f
  plantuml`), it gives the same PlantUML as the source DSL does. That
  check is run once to make the reference output; the test compares
  c4rizer's own read-back.
- **P3-414** The JSON holds every element, relationship (implied and
  copied ones included), view, style, theme reference, property,
  documentation section and decision of the model. Each view element
  carries the scene's box centre as `x`, `y` (px, top-left as
  Structurizr expects) and each relationship view carries its vertices
  (P3-318).
- **P3-415** The JSON's workspace `properties` hold `structurizr.dsl`,
  the base64 of the root file's bytes, and
  `structurizr.inspection.error|warning|info|ignore`, the violation
  counts of `inspect` *(probed)*.
- **P3-416** A file whose first non-whitespace byte is `{` is read as
  Structurizr workspace JSON by `render`, `lint`, `export`, `validate`,
  `inspect` and `list`. The model it gives equals the model of the DSL it
  was exported from: same element names, kinds, parents, tags,
  relationships, views and styles.
- **P3-417** A JSON workspace keeps its element positions as `at` and
  `size` in the built views, so `render` of an exported JSON gives the
  same SVG as the source DSL.
- **P3-418** Malformed JSON, or JSON that is not a workspace, is a parse
  error naming the file, line and column (SRS 5.1). An unknown key is
  ignored.
- **P3-419** `workspace extends <relative .json file>` uses that JSON as
  the base (closes the JSON part of the deferred P2-16 row).
- **P3-420** The editor opens a JSON workspace read-only: views draw and
  export, while moves, resizes, text, style and vertex edits are refused
  with one status-bar message saying the file is JSON. The editor never
  writes JSON, which would be a second source of truth (SRS 1).

**Theme and static site**

- **P3-421** `theme` writes `workspace-theme.json` with `name`,
  `description`, and the `elements` and `relationships` style arrays of
  the workspace's own styles (not those from themes) in Structurizr theme
  format. It equals the reference output on the styles fixture
  `tests/data/styles/`.
- **P3-422** A theme written by P3-421 and loaded with `theme <file>`
  gives the same resolved styles as the source workspace.
- **P3-423** `static` writes c4rizer's own site, not Structurizr's JS
  viewer: a folder with `index.html`, one `<key>.svg` per drawable view
  (P2-659 `name` mode), and one HTML page per view. It needs no script
  and no network, and vendors no JS: every page passes the EXP-1 scan.
  The parity table records this as "differs (accepted)".
- **P3-424** In the static site, `index.html` lists every view in view
  list order (EDIT-8) with its title and key. Each view page links its
  enterable boxes (EDIT-15) to the views they open, and links back to
  the index.

**merge and generate (P3-8)**

- **P3-8** `merge` applies the layout of a Structurizr workspace JSON to
  a workspace, and `generate system-landscape` merges several
  workspaces into one landscape. Split into P3-425..P3-430.
- **P3-425** `c4rizer merge -w FILE -l LAYOUT.json [-v KEY] -o OUT`
  writes to OUT a copy of FILE whose views carry the `at`, `size` and
  `via` lines that `import-layout` would add from LAYOUT. With `-v`,
  only that view changes. Spellings follow Structurizr (`-w`, `-l`,
  `-v`, `-o`, and the long forms).
- **P3-426** `merge` never changes FILE. OUT equals FILE except for the
  added or rewritten `at`/`size`/`via` lines.
- **P3-427** `merge` matches elements by Structurizr canonical name
  (Structurizr's `DefaultLayoutMergeStrategy`), and an element of FILE
  missing from LAYOUT keeps its current position or auto-placement.
- **P3-428** `c4rizer generate system-landscape -i DIR -o OUT.dsl`
  reads every `*.dsl` and `*.json` workspace in DIR (sorted by path) and
  writes one DSL file holding every person and software system once
  (matched by name), the relationships between them, and one
  `systemLandscape` view with `include *`.
- **P3-429** In `generate`, a software system that appears in several
  input workspaces keeps the description of the first one that has a
  non-empty description.
- **P3-430** `merge` and `generate` exit 2 on usage errors and 1 on a
  parse error of any input, naming the file.

**Editor and animation**

- **P3-431** The editor's export dialog offers every format of P3-400.
  "Export view" writes the open view's file and "Export all views"
  writes every file into the chosen folder (EDIT-7 folder rule).
- **P3-432** `export -f svg|png --animation` writes per-step images as
  P3-225 says. `--mode dark` follows P2-21 for every image format.

explore

Authority: SRS 4.7 P3-9; parity rows UI "Graph exploration" and
"Documentation and decisions viewer" (content parsed in phase 2,
`!docs`/`!adrs`, `Model::docs`, `Element::docs`); Structurizr pages
`explore.jsp` (graph for static and custom views, tree for static and
deployment views), `graph.jsp`, `tree.jsp`, `model.jsp`,
`documentation.jsp`, `decisions.jsp` (read from the 6.2.4 war). Fixture
`tests/data/phase3/explore.dsl` with `!docs` and `!adrs` folders.

- **P3-9** The editor offers model exploration (tables, search, usages,
  graph and tree) and a viewer for the documentation and decisions
  attached by `!docs` and `!adrs`. Split into P3-500..P3-517.

**Model tables and search**

- **P3-500** An "Explore" panel lists the static model nested as
  Structurizr's Model page does: custom elements, people, software
  systems with their containers and components. One deployment section
  follows per environment, nested by node. Each row shows the id, name,
  type line, description and tags.
- **P3-501** Each element row expands to show its relationships, with
  their direction, other end, description and technology. Implied
  relationships are marked "implied" (Model page `implies`).
- **P3-502** A relationships table lists every relationship of the
  model, implied and copied ones included, with source, destination,
  description, technology and tags. It sorts by any column, and every
  row's id comes from the model.
- **P3-503** A search field filters the tables to elements and
  relationships whose name, id, description, technology, tag or property
  value contains the typed text, case-insensitively. It updates within
  one frame for a 1000-element model (`perf_p3_503`).
- **P3-504** Selecting an element in the tables shows its usages: every
  view that draws it, with key and title, and, for each, whether it is
  drawn directly or through lifting (CORE-5). A click on a usage opens
  that view with the box selected.
- **P3-505** An element that no view draws is marked "in no view" in
  the tables, matching inspection `model.element.noview` (P2-6xx
  `ElementNoView`).
- **P3-506** "Reveal in file" on any row copies
  `<path>:<line>:<column>` of the declaration to the clipboard (the app
  has no text editor, EDIT-21).
- **P3-507** The explore panel is read-only: nothing in it writes the
  file.

**Graph and tree**

- **P3-508** "Graph" for a static, custom or landscape view draws the
  view's elements and relationships as a node-link graph with no
  boundaries, laid out by a deterministic force layout (same positions
  on every run, CORE-25).
- **P3-509** In the graph, a click on a node keeps it, its neighbours
  and their links fully opaque and fades every other node and link to
  10% (graph.jsp `showConnectedNodes`). A click on empty space clears
  this.
- **P3-510** "Tree" for a static view draws the scope hierarchy of its
  elements (system > container > component, people and external
  systems as roots). For a deployment view it draws environment > node >
  instance, laid out left to right (tree.jsp).
- **P3-511** Graph and tree are offered for exactly the kinds
  Structurizr offers. Graph: custom, landscape, context, container and
  component views. Tree: the same plus deployment. Neither is offered
  for dynamic, filtered or image views.
- **P3-512** Graph and tree pan and zoom with EDIT-28 gestures, and a
  double click on a node opens the diagram view with that box selected.

**Documentation and decisions**

- **P3-513** A "Documentation" view lists the sections attached by
  `!docs` to the workspace and to each software system, in file-name
  order (P2 `!docs` rule), and shows the selected section rendered.
- **P3-514** Markdown sections are rendered, with `pulldown-cmark` in
  the editor crate only, with headings, paragraphs, emphasis, lists,
  code blocks, tables and links. Images with a relative path are drawn
  from the docs folder, and remote images are not fetched unless network
  (P3-600) is on.
- **P3-515** AsciiDoc sections are shown as their source text in a
  monospace block, with a one-line note that AsciiDoc is not rendered
  (no maintained Rust AsciiDoc renderer exists).
- **P3-516** A "Decisions" view lists the decisions attached by `!adrs`
  with id, title, date and status, sorted by id. The selected decision is
  rendered as P3-514 says, and the links between decisions ("Supersedes
  2") open the linked decision.
- **P3-517** An embedded diagram reference in Markdown, `![](embed:<view
  key>)`, draws that view's scene inline at its fitted size. An unknown
  key shows a one-line placeholder naming it, which matches inspection
  `documentation.embeddedview`.

network

Authority: ROADMAP row `network` ("HTTPS themes, `!include`, `workspace
extends`, image URLs, with a disk cache; skipped-URL notice names the
URL and what was not drawn"); phase2-decisions Part C; deferred rows
P2-138/P2-16, P2-322, P2-409, P2-429, P2-603, P2-321 extra. Today an
HTTPS `!include`/`extends` is `UrlNotSupported`, and URL and named
themes, URL icons and URL images give a notice or placeholder.

**Opt-in**

- **P3-600** No build of c4rizer opens a network connection unless the
  user turned network on for that run: `--network` on `render`, `lint`,
  `export`, `validate`, `inspect`, `list`, `merge` and `generate`, or
  the editor's "Allow network for this file" switch.
- **P3-601** With network off, behaviour is exactly phase 2:
  - an HTTPS `!include` or `workspace extends` is `UrlNotSupported`;
  - URL themes, named themes other than bundled `default`, URL icons and
    URL images give the phase-2 placeholder or notice.

  No cache entry is read either, so a build depends only on the files on
  disk and is the same on every machine (CORE-25); the cache serves only
  failed fetches (P3-611).
- **P3-602** The editor's network switch is remembered per diagram file
  (EDIT-13 recent-file state), is off for a file never opened before,
  and shows a mark in the status bar while it is on.

**What is fetched**

- **P3-603** With network on, these are fetched:
  - `!include <https url>`, inlined as P2-5 says;
  - `workspace extends <https url>`, DSL or JSON (P3-419);
  - `theme <https url>` / `themes`;
  - a named theme as
    `https://static.structurizr.com/themes/<name>/theme.json`
    (Structurizr's own location, `DEFAULT_THEME_URL` pattern);
  - `icon <https url>` in element styles and themes;
  - `image <https url>` in image views;
  - `-workspace <https url>` (P2-603).
- **P3-604** Only `https://` URLs are fetched. Any other scheme is an
  error naming the URL (P2-138, kept), and a redirect to a non-HTTPS URL
  is refused with an error naming both URLs.
- **P3-605** A fetch follows at most 5 redirects, times out after 10 s,
  and refuses a body over 10 MiB, each with an error naming the URL and
  the limit.
- **P3-606** A relative `!include`, icon or image path inside a fetched
  file resolves against that file's URL. It is still fetched only over
  HTTPS, and it never resolves to a local path.
- **P3-607** A fetched `!include` that includes itself, directly or
  through others, stops with the P2-137 cycle error naming the URL.
- **P3-608** PlantUML, Mermaid and Kroki image sources and their
  `*.url` properties are never fetched, even with network on (parity
  row: never). They keep their P2-321 placeholder.

**Cache**

- **P3-609** Every successful fetch is stored in
  `$XDG_CACHE_HOME/c4rizer/net/` (default `~/.cache/c4rizer/net/`),
  one file per URL plus its fetch time. A file is named by an in-house
  FNV-1a hash of its URL, and an index file in that folder maps each
  hash to its URL (no hash crate).
- **P3-610** With network on, a URL is fetched again on every build
  that needs it. The editor's rebuild after a local save reuses the
  bytes fetched at file open (EDIT-29 timing is not affected by the
  network).
- **P3-611** With network on and a fetch that fails (no connection,
  timeout, HTTP status not 2xx), the cached copy is used when one
  exists, and a notice names the URL and the cached copy's fetch date.
  With no cached copy, the construct falls back to its P3-601 behaviour
  and the notice says that the fetch failed.
- **P3-612** `c4rizer cache clear` deletes the cache folder and exits 0.
  `c4rizer cache list` prints one line per cached URL with its fetch
  time and size.

**Notices and editing**

- **P3-613** Every URL that was not drawn (network off, fetch failed
  with no cache, or a never-fetched source) gives one notice naming the
  URL and what was not drawn ("theme styles", "icon of tag X", "image of
  view K", "included file"). The notice kind is the new `NotFetched`, not
  `NoEffect`.
- **P3-614** A text edit (DSL-10) or `at`/`size`/`via` write that would
  land in a fetched file is refused with a status-bar message naming the
  URL. No file changes.
- **P3-615** The editor watches local files only. A fetched file is
  re-read on an explicit "Reload" and on the next file open, never by
  polling.

**Tests and build**

- **P3-616** No test opens a network connection. Fetching goes through
  a `Loader` method that tests implement with in-memory bytes. A test
  using the real fetcher is `#[ignore]` and named `net_*`.
- **P3-617** The real fetcher uses `ureq` with `rustls` (blocking, no
  async runtime, pure-Rust TLS) and lives behind the core cargo feature
  `network`. The CLI and the editor enable it by default; core's tests
  never do. A build without it treats `--network` as a usage error
  saying that the build has no network support (exit 2), as `--png`
  without `png` does.
- **P3-618** A fetched SVG icon or image passes P2-340 and EXP-1 like a
  local one: it is embedded as a data URI and nothing in the export
  refers to the URL.
- **P3-619** The scene and every export of a file built with network
  on, from a fully cached state, are byte-identical across two runs
  (CORE-25), and identical to a build whose loader serves the same
  bytes from memory.
- **P3-620** `render`, `lint` and the editor print or show which URLs
  were fetched, one line each (`fetched <url> (<n> bytes)` on stderr for
  the CLI), so the user can see every connection made.
- **P3-621** Bundled `theme default` (styles-themes) is used without a
  fetch whether network is on or off.

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
