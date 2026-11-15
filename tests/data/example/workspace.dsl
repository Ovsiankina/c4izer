// Structurizr DSL. C1 and C2 at step 1, C3 at step 3.
// This file is the model: every element, relationship and colour lives here.
// Render it into docs/c4/ with:
//     python3 docs/c4/c4.py docs/workspace.dsl docs/c4/layout.json -o docs/c4
// (docs/c4/layout.json only holds box positions; see docs/c4/README.md.)
// Source design: docs/superpowers/specs/2026-11-15-atomanja-design.md

workspace "Atomanja" "Atomanja exists to put financial pressure on your tasks. Be accounted for. Be more productive." {

    model {
        // People are roles, never named individuals.
        worker = person "Worker" "The one person whose tasks are tracked. Sees own tasks and debts, marks debts as sent."
        witness = person "Witness" "Accountability partner. Sees late tasks, claims debts, marks them received."
        admin = person "Admin witness" "Maintainer. Troubleshoots the witness view on fake data."

        vikunja = softwareSystem "Vikunja" "Existing task manager. Source of tasks and deadlines. Never modified." "External"
        push = softwareSystem "Push services" "Browser vendors' endpoints that deliver notifications to installed apps." "External"
        smtp = softwareSystem "Mail server" "Delivers invite and password-reset emails." "External"

        system = softwareSystem "Atomanja" "Records task deadlines, turns missed ones into debts, and tracks their settlement." {
            app = container "Atomanja server" "Single binary. Polls Vikunja, keeps the ledger, serves both views, sends notifications and emails." "Rust, axum, HTML + htmx" {
                // C3: one component per module (step 3, 2026-11-15).
                // Tags group related components; each tag has one colour.
                cli = component "cli" "Command line: serve, create accounts, generate push keys, seed dev data." "Rust binary" "Entry"
                web = component "web" "HTTP layer: both views, login flows, debt actions, push subscription." "Rust, axum, htmx" "Entry"
                sync = component "sync" "Poll loop: fetch, apply to ledger, persist, notify." "Rust, tokio" "Entry"
                devdata = component "devdata" "Fills the dev database with fake data through the real ledger rules." "Rust module" "Entry"

                ledger = component "ledger" "Pure domain rules: deadlines, met or late, debts, claims, marks, amount." "Rust module" "Core"
                auth = component "auth" "Pure account rules: roles, password hashing, tokens, permission checks." "Rust module" "Core"
                config = component "config" "Loads every setting from environment variables at startup." "Rust module" "Core"

                vikunjaClient = component "vikunja" "Read-only Vikunja client. Maps tasks to snapshot records." "Rust, HTTP client" "Adapter"
                notify = component "notify" "Turns ledger changes into push notifications. Owns the push keys." "Rust, Web Push" "Adapter"
                mail = component "mail" "Sends invite and password-reset emails." "Rust, SMTP client" "Adapter"
                store = component "store" "Persistence for ledger and dev databases. Ledger is insert-only." "Rust, SQLite" "Adapter"
            }
            db = container "Ledger database" "Append-only record of deadlines, debts, claims, marks and accounts." "SQLite file" "Database"
            devdb = container "Dev database" "Fake tasks and debts for the admin witness. Isolated from the real ledger." "SQLite file" "Database"
        }

        // C1 / C2 relationships. Each line is one direction and says its intent.
        worker -> app "Views tasks and debts, marks debts sent, invites witnesses" "HTTPS"
        witness -> app "Views late tasks, claims debts, marks debts received" "HTTPS"
        admin -> app "Troubleshoots the witness view on fake data" "HTTPS"

        app -> vikunja "Polls tasks and deadlines, read-only" "HTTPS/JSON"
        app -> db "Reads and appends ledger entries" "SQL"
        app -> devdb "Reads fake data for the admin witness" "SQL"
        app -> push "Sends notifications through" "Web Push"
        app -> smtp "Sends invite and reset emails through" "SMTP"

        push -> worker "Delivers notifications to"
        push -> witness "Delivers notifications to"

        // C3 relationships (match the `Uses:` lines in src/*.rs).
        worker -> web "Views tasks, marks debts sent" "HTTPS"
        witness -> web "Claims debts, marks them received" "HTTPS"
        admin -> web "Troubleshoots on fake data" "HTTPS"

        cli -> web "Starts HTTP server"
        cli -> sync "Starts poll loop"
        cli -> devdata "Runs seeding"
        cli -> store "Opens databases with"
        cli -> auth "Creates accounts with"
        cli -> notify "Generates push keys with"

        web -> config "Reads settings from"
        web -> ledger "Runs debt actions on"
        web -> auth "Checks sessions and permissions with"
        web -> store "Loads and saves through"
        web -> notify "Stores subscriptions, notifies through"
        web -> mail "Sends invites and resets through"

        sync -> config "Reads settings from"
        sync -> vikunjaClient "Fetches snapshot through"
        sync -> ledger "Applies snapshot to"
        sync -> store "Appends changes through"
        sync -> notify "Hands changes to"

        devdata -> ledger "Builds fake debts with"
        devdata -> auth "Builds fake accounts with"
        devdata -> store "Writes dev data through"

        vikunjaClient -> config "Reads settings from"
        vikunjaClient -> vikunja "Fetches tasks from" "HTTPS/JSON"
        notify -> config "Reads settings from"
        notify -> ledger "Reads change types from"
        notify -> auth "Picks recipients by role with"
        notify -> push "Sends notifications through" "Web Push"
        mail -> config "Reads settings from"
        mail -> smtp "Sends emails through" "SMTP"
        store -> ledger "Persists state of"
        store -> auth "Persists accounts of"
        store -> db "Reads and appends" "SQL"
        store -> devdb "Reads and writes" "SQL"
    }

    views {
        // C1: people and neighbouring systems only. No protocols, no
        // delivery infrastructure (push, mail): those appear from C2 down.
        systemContext system "C1" "System Context diagram for Atomanja" {
            include *
            exclude push smtp
            autoLayout
        }

        container system "C2" "Container diagram for Atomanja" {
            include *
            autoLayout
        }

        // C3 is split in two so that neither view is a tangle. Together
        // they show every component and every relationship.
        component app "C3-serving" "Component diagram for the Atomanja server: serving and polling" {
            include *
            exclude cli config devdata
            autoLayout
        }

        component app "C3-startup" "Component diagram for the Atomanja server: startup, settings and dev data" {
            include cli config devdata sync notify web auth store vikunjaClient mail ledger
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
            element "Container" {
                background #1e88e5
                color #ffffff
            }
            element "Database" {
                shape Cylinder
                background #00796b
                color #ffffff
            }
            element "Entry" {
                background #c2410c
                color #ffffff
            }
            element "Core" {
                background #2e7d32
                color #ffffff
            }
            element "Adapter" {
                background #1e88e5
                color #ffffff
            }
            relationship "Relationship" {
                routing Orthogonal
            }
        }
    }
}
