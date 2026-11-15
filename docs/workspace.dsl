// Structurizr DSL. C1 and C2 at step 1, C3 at step 3.
// This file is the model: every element, relationship and colour lives here.
// Until c4rizer can render itself, render it with the prototype:
//     python3 ~/Projects/c4-design/scripts/c4.py docs/workspace.dsl docs/c4/layout.json -o docs/c4
// (docs/c4/layout.json only holds box positions and view options.)

workspace "c4rizer" "c4rizer exists so C4 diagrams can be written as text in one file, drawn cleanly at once, and arranged by hand without a heavy server or an auto-layout that locks editing." {

    model {
        // People are roles, never named individuals.
        architect = person "Architect" "Writes the model and arranges the diagrams until they read at a glance."
        agent = person "Coding agent" "Edits the model and renders and lints diagrams from the command line, without a screen."

        textEditor = softwareSystem "Text editor" "Whatever editor the architect already uses for the diagram file. Half of hot reload." "External"

        system = softwareSystem "c4rizer" "Turns one text file into clean C1-C3 diagrams, lints them, and lets them be arranged by hand." {
            core = container "c4rizer core" "Parses, places, routes and lints; builds a scene per view; turns scenes into SVG and PNG. No UI, no file I/O." "Rust library" "Core" {
                // C3: one component per module (step 3, 2026-11-15). Tags group families.
                cGeom = component "geom" "Grid units, points, rects, octilinear segments and crossing tests." "Rust module" "Layout"
                cText = component "text" "Lossless lexer and syntax tree; printing gives the input back." "Rust module" "Parse"
                cModel = component "model" "Elements, relationships, styles and view specs, each with its source span." "Rust module" "Parse"
                cEdit = component "edit" "Turns moves, resizes, text and style edits into byte patches on the file." "Rust module" "Parse"
                cView = component "view" "Resolves a view spec into boxes and edges: include, exclude, lifting, focus." "Rust module" "Layout"
                cPlace = component "place" "Gives every unplaced box a grid position without moving placed ones." "Rust module" "Layout"
                cRoute = component "route" "Octilinear grid router: bus lanes, joins, fewest crossings, rip-up and retry." "Rust module" "Layout"
                cLabel = component "label" "Places each line label clear of boxes, labels and lines." "Rust module" "Layout"
                cScene = component "scene" "Builds the plain-data picture of a view, every item tagged with its model id." "Rust module" "Output"
                cLint = component "lint" "Checks a scene against the C4 rules and the crossing budget." "Rust module" "Output"
                cExport = component "export" "Writes a scene as SVG text or PNG bytes." "Rust module" "Output"
                cPipeline = component "pipeline" "Entry points: one view's scene and lint report, or every view." "Rust module" "Entry"
            }
            cli = container "c4rizer CLI" "Exports views to SVG and PNG and lints them with an exit code, for scripts and agents." "Rust binary" "App"
            editor = container "c4rizer editor" "Desktop canvas: reloads on save, drag boxes with live re-routing, edit text in place." "Rust, egui" "App"

            model = container "Diagram file" "Single source of truth: elements, relationships, styles, views and box positions." "c4rizer DSL" "File"
            exports = container "Diagram exports" "Rendered views for docs and reviews." "SVG, PNG" "File"
        }

        // C1 / C2 relationships. Each line is one direction and says its intent.
        architect -> editor "Watches live, arranges boxes and edits text with"
        architect -> textEditor "Writes the model and views with"
        textEditor -> model "Saves the diagram file" "file I/O"
        agent -> cli "Renders and lints diagrams with" "shell, exit code"
        agent -> model "Edits the model and views in" "file tools"

        cli -> core "Renders and lints views through" "Rust API"
        editor -> core "Renders, re-routes and lints views through" "Rust API"

        cli -> model "Reads the diagram file" "file I/O"
        cli -> exports "Writes SVG and PNG to" "file I/O"
        editor -> model "Reads, and writes moves and edits back to" "file I/O, atomic"
        editor -> exports "Writes SVG and PNG to" "file I/O"

        // C3 relationships (match the `Depends on:` lines in crates/core/src/*.rs).
        cli -> cPipeline "Builds scenes and lint reports with" "Rust API"
        cli -> cExport "Turns scenes into files with" "Rust API"
        editor -> cPipeline "Builds and re-routes scenes with" "Rust API"
        editor -> cEdit "Writes canvas edits back with" "Rust API"
        editor -> cExport "Exports the open view with" "Rust API"

        cPipeline -> cModel "Parses the file with"
        cPipeline -> cView "Resolves views with"
        cPipeline -> cPlace "Places unplaced boxes with"
        cPipeline -> cRoute "Routes lines with"
        cPipeline -> cLabel "Places labels with"
        cPipeline -> cScene "Builds scenes with"
        cPipeline -> cLint "Checks scenes with"

        cModel -> cText "Builds on the syntax tree of"
        cEdit -> cText "Patches source spans from"
        cEdit -> cModel "Finds edit targets in"
        cView -> cModel "Reads view specs and elements from"
        cPlace -> cView "Fills positions into plans of"
        cRoute -> cView "Routes the edges of plans of"
        cLabel -> cRoute "Places labels along routes of"
        cLabel -> cView "Avoids boxes of plans of"
        cScene -> cView "Draws boxes of plans of"
        cScene -> cRoute "Draws lines of"
        cScene -> cLabel "Draws labels of"
        cLint -> cScene "Measures geometry of"
        cLint -> cModel "Checks C4 rules against"
        cLint -> cText "Reads source spans from"
        cExport -> cScene "Serialises"

        cView -> cGeom "Sizes boxes on the grid of"
        cPlace -> cGeom "Snaps positions to the grid of"
        cRoute -> cGeom "Walks the grid of"
        cPlace -> cModel "Reads element ids and kinds from"
        cRoute -> cModel "Reads box shapes from"
        cLabel -> cGeom "Tests overlaps with"
        cScene -> cGeom "Converts grid units with"
    }

    views {
        // C1: people and neighbouring systems only. No protocols.
        systemContext system "C1" "System Context diagram for c4rizer" {
            include *
            autoLayout
        }

        container system "C2" "Container diagram for c4rizer" {
            include *
            autoLayout
        }

        // C3 is split in three so that none is a tangle; together they show
        // every component and every relationship.
        component core "C3-stages" "Component diagram for c4rizer core: pipeline stages" {
            include cText cModel cEdit cView cPlace cRoute cLabel cScene cLint cExport
            autoLayout
        }
        component core "C3-entry" "Component diagram for c4rizer core: entry points" {
            include cli editor cPipeline cEdit cExport cModel cView cPlace cRoute cLabel cScene cLint
            autoLayout
        }
        component core "C3-geom" "Component diagram for c4rizer core: geometry" {
            include cGeom cView cPlace cRoute cLabel cScene
            autoLayout
        }

        // One colour family per kind of thing; related boxes share a colour.
        styles {
            element "Person" {
                shape Person
                background #0b3c6f
                color #ffffff
            }
            element "Software System" {
                background #1565c0
                color #ffffff
            }
            element "External" {
                background #5d5a6b
                color #ffffff
            }
            element "Core" {
                background #2e7d32
                color #ffffff
            }
            element "App" {
                background #c2410c
                color #ffffff
            }
            element "Parse" {
                background #6a1b9a
                color #ffffff
            }
            element "Layout" {
                background #1565c0
                color #ffffff
            }
            element "Output" {
                background #00838f
                color #ffffff
            }
            element "Entry" {
                background #c2410c
                color #ffffff
            }
            element "File" {
                shape Cylinder
                background #00796b
                color #ffffff
            }
        }
    }
}
