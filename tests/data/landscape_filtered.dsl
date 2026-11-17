workspace "Estate" "A shop estate, as the filtered-view, custom-view and image-view cookbook pages draw theirs." {

    model {
        operator = person "Operator" "Runs the shop day to day." "Tag 1"
        auditor = person "Auditor" "Checks the books every quarter." "Tag 2"
        mail = softwareSystem "Mail" "Sends order and audit mail." "External,Tag 2"
        ledger = softwareSystem "Ledger" "Keeps the books."
        shop = softwareSystem "Shop" "Sells goods online." "Tag 1" {
            web = container "Web" "Serves the shop pages." "Rust" "Tag 1" {
                api = component "Api" "Answers page calls." "Rust"
                auth = component "Auth" "Checks who is signed in." "Rust"
            }
            db = container "Db" "Stores orders." "SQLite" "Database"
        }
        board = element "Board" "Whiteboard" "Holds the release plan."
        pin = element "Pin" "Sticky note" "Marks one task."

        operator -> web "Manages orders in" "HTTPS" "Tag 1"
        auditor -> ledger "Audits the books in" "HTTPS" "Tag 2"
        api -> mail "Sends order mail through" "SMTP" "Tag 2"
        auth -> mail "Sends sign-in codes through" "SMTP"
        api -> db "Reads and writes orders in" "SQL"
        posts = shop -> ledger "Posts daily sales to" "HTTPS" "Tag 1"
        board -> pin "Holds"
    }

    views {
        systemLandscape "landscape" "The estate" {
            include *
            default
            technology off
            at operator 200 100
            at auditor 700 100
            at shop 200 450
            at ledger 700 450
            at mail 450 800
        }

        filtered "landscape" include "Tag 1,Tag 2,Relationship" "landscape-tagged" "Tagged elements and their lines"
        filtered "landscape" exclude "Tag 1" "landscape-untagged" "Everything not tagged Tag 1"
        filtered "landscape" include "Element,Relationship" "landscape-all" "The estate, unfiltered"

        container shop "containers" "Inside the shop" {
            include *? "->mail"
            exclude "relationship.tag==Tag 2"
        }

        custom "board" "The release board" "Custom elements only" {
            include *
        }

        image shop "shop-picture" {
            image landscape_filtered/shop.svg
            title "The shop, drawn by hand"
        }

        image * "flow" {
            plantuml landscape_filtered/flow.puml
            title "The order flow"
        }
    }
}
