// c4rizer diagram file: the prototype example (tests/data/example/workspace.dsl
// plus layout.json) converted once by hand (DSL-12). Model and styles are
// unchanged; each view carries its layout as c4rizer lines: subtitle, options,
// `at` (box centre) and `size`. View keys as in the prototype.

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
            subtitle "Who uses Atomanja and which other systems it depends on."
            include *
            exclude push smtp
            technology off
            maxCrossings 0
            at worker 220 170
            at witness 620 170
            at admin 1020 170
            at system 620 480
            at vikunja 620 760
            size system 280 140
        }

        container system "C2" "Container diagram for Atomanja" {
            subtitle "The running parts of Atomanja and the technology each one uses."
            include *
            maxCrossings 0
            at push 150 230
            at worker 520 230
            at witness 800 230
            at admin 1080 230
            at app 800 540
            at db 660 780
            at devdb 940 780
            at vikunja 150 780
            at smtp 1410 780
            size app 440 140
            size db 200 140
            size devdb 200 140
            size push 200 140
            size vikunja 200 140
            size smtp 220 140
        }

        // C3 is split in two so that neither view is a tangle. Together
        // they show every component and every relationship.
        component app "C3-serving" "Component diagram for the Atomanja server: serving and polling" {
            subtitle "What runs on every request and every poll. cli, config and devdata are on the startup diagram."
            include *
            exclude cli config devdata
            bus on
            labelWidth 16
            maxCrossings 1
            hide push -> worker
            hide push -> witness
            at worker 720 190
            at witness 1000 190
            at admin 1280 190
            at web 1000 480
            at mail 1510 480
            at smtp 1850 480
            at push 150 760
            at notify 490 760
            at auth 830 760
            at ledger 1170 760
            at store 1510 760
            at db 1850 760
            at vikunja 150 1040
            at vikunjaClient 490 1040
            at sync 1000 1040
            at devdb 1850 1040
            size vikunja 200 140
            size push 200 140
            size smtp 200 140
            size db 200 140
            size devdb 200 140
        }

        component app "C3-startup" "Component diagram for the Atomanja server: startup, settings and dev data" {
            subtitle "What cli starts, who reads config, and what devdata seeds. Only lines touching those three are drawn here."
            include cli config devdata sync notify web auth store vikunjaClient mail ledger
            bus on
            labelWidth 16
            maxCrossings 0
            focus cli config devdata
            at cli 1050 200
            at sync 200 480
            at notify 540 480
            at web 880 480
            at auth 1220 480
            at store 1560 480
            at devdata 1900 480
            at vikunjaClient 200 760
            at config 540 760
            at mail 880 760
            at ledger 1900 760
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
