workspace "Groups" "Groups on every level, auto-placed (P2-130, P2-132)" {
    model {
        properties {
            "structurizr.groupSeparator" "/"
        }

        group "Customers" {
            buyer = person "Buyer" "Buys goods."
            seller = person "Seller" "Sells goods."
        }
        operator = person "Operator" "Runs the shop."

        group "Partners" {
            bank = softwareSystem "Bank" "Takes payments." {
                tags "External"
            }
            courier = softwareSystem "Courier" "Delivers parcels." {
                tags "External"
            }
        }
        mail = softwareSystem "Mail" "Sends e-mail." {
            tags "External"
        }

        shop = softwareSystem "Shop" "Sells goods online." {
            group "Front" {
                web = container "Web" "Serves pages." "Rust"
                group "Apps" {
                    mobile = container "Mobile" "Phone app." "Kotlin"
                    desktop = container "Desktop" "Desktop app." "Rust"
                }
            }
            api = container "Api" "Handles orders." "Rust" {
                group "Orders" {
                    orders = component "Orders" "Takes orders." "Rust"
                    basket = component "Basket" "Holds items." "Rust"
                }
                payments = component "Payments" "Calls the bank." "Rust"
                notify = component "Notify" "Sends mail." "Rust"
            }
            group "Storage" {
                db = container "Db" "Stores orders." "Postgres" {
                    tags "Database"
                }
                cache = container "Cache" "Caches pages." "Redis"
            }
        }

        buyer -> web "Browses"
        buyer -> mobile "Orders with"
        seller -> desktop "Lists goods with"
        operator -> web "Manages"
        web -> orders "Places orders with" "HTTPS"
        mobile -> orders "Places orders with" "HTTPS"
        desktop -> basket "Edits listings with" "HTTPS"
        web -> cache "Caches pages in" "RESP"
        orders -> basket "Fills"
        orders -> payments "Pays through"
        orders -> notify "Notifies through"
        orders -> db "Stores orders in" "SQL"
        basket -> db "Loads items from" "SQL"
        payments -> bank "Charges cards with" "HTTPS"
        notify -> mail "Sends e-mail with" "SMTP"
        api -> courier "Books parcels with" "HTTPS"
    }

    views {
        systemContext shop "context" {
            include *
            title "Shop context"
            technology off
        }
        container shop "containers" {
            include *
            title "Shop containers"
        }
        component api "components" {
            include *
            title "Api components"
        }
    }
}
