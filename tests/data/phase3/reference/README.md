# Structurizr 6.2.4 reference outputs (exporters row, SRS 4.7 P3-406..P3-421)

Generated once, on 2026-11-18, by `generate.sh` in this folder from the
installed Structurizr 6.2.4 (CLI v2026.10.26,
`~/.local/share/structurizr/structurizr.war`). Tests compare c4rizer's
output with these files; they never run Java or touch the network.

## Fixtures

| folder | source fixture |
|---|---|
| `example/` | `tests/data/example.dsl` |
| `deployment/` | `tests/data/phase3/deployment.dsl` |
| `dynamic/` | `tests/data/phase3/dynamic.dsl` |
| `styles/` | `tests/data/styles/all_styles.dsl` |

Each folder's `structurizr.dsl` is the file Structurizr was given: the
fixture with its c4rizer-only view lines removed (`subtitle`,
`technology`, `bus`, `labelWidth`, `maxCrossings`, `at`, `size`, `hide`,
`focus`, `via`, `line`), since Structurizr refuses them. They are layout
and drawing aids and change no text export (see "c4rizer-only lines"
below). For `styles/`, the `theme`/`themes` lines are removed too: the
theme export holds the workspace's own styles only, and `theme default`
would make Structurizr fetch a URL.

## How each format was produced

| folder | how |
|---|---|
| `plantuml-structurizr/` | `structurizr export -w structurizr.dsl -f plantuml/structurizr` |
| `plantuml-c4plantuml/` | `structurizr export -w structurizr.dsl -f plantuml/c4plantuml` |
| `mermaid/` | `structurizr export -w structurizr.dsl -f mermaid` |
| `websequencediagrams/` | `structurizr export -w structurizr.dsl -f websequencediagrams` |
| `json/` | `structurizr export -w structurizr.dsl -f json` |
| `dot/` | `Probe.java`: `new DOTExporter().export(workspace)` from `structurizr-export-6.2.4.jar` (the CLI does not expose it), one `structurizr-<key>.dot` per diagram |
| `ilograph/` | `Probe.java`: `new IlographExporter().export(workspace)`, written as `workspace.idl.yaml` (P3-412) |
| `styles/all_styles-theme.json` | `structurizr export -w structurizr.dsl -f theme` |
