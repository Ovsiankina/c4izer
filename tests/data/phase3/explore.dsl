workspace "Shop" "An online shop, explored." {

    !docs explore/docs

    model {
        customer = person "Customer" "Buys goods." "Buyer"
        clerk = person "Clerk" "Packs orders."
        sensor = element "Sensor" "Probe" "Measures the warehouse." {
            properties {
                "location" "Dock 3"
            }
        }
        scanner = element "Scanner" "Probe" "Reads parcel labels."
        shop = softwareSystem "Shop" "Sells goods." {
            !docs explore/shop-docs
            !adrs explore/adrs
            web = container "Web" "Serves the pages." "Rust" {
                cart = component "Cart" "Holds items." "Rust module"
                pay = component "Payments" "Charges cards." "Rust module"
            }
            db = container "Database" "Stores orders." "PostgreSQL" "Database"
        }
        bank = softwareSystem "Bank" "Moves money." "External"
        archive = softwareSystem "Archive" "Keeps old orders." "Legacy"

        customer -> web "Orders through" "HTTPS"
        clerk -> shop "Packs orders with"
        web -> db "Reads and writes" "SQL"
        pay -> bank "Charges cards through" "HTTPS" "Payment"
        cart -> pay "Checks out with"
        sensor -> shop "Reports stock to" "MQTT"
        scanner -> sensor "Wakes"

        live = deploymentEnvironment "Live" {
            cloud = deploymentNode "Cloud" "The cloud account." "AWS" {
                gate = infrastructureNode "Gateway" "Routes requests." "ELB"
                host = deploymentNode "Host" "Runs the shop." "EC2" {
                    webLive = containerInstance web
                }
                store = deploymentNode "Store" "Managed database." "RDS" {
                    dbLive = containerInstance db
                }
            }
            gate -> webLive "Forwards to" "HTTPS"
        }
    }

    views {
        systemLandscape "landscape" "Everything" {
            include *
            exclude archive
        }
        systemContext shop "context" "The shop and its users" {
            include *
        }
        container shop "containers" "Inside the shop" {
            include *
        }
        component web "components" "Inside the web app" {
            include *
        }
        filtered "containers" include "Database" "databases" "The databases"
        custom "devices" "Devices" {
            include sensor scanner
        }
        deployment shop "Live" "live" "The shop in production" {
            include *
        }
        dynamic shop "checkout" "How a checkout runs" {
            customer -> web "Orders through"
            web -> db "Reads and writes"
        }
        styles {
            element "Database" {
                shape Cylinder
            }
            element "Person" {
                shape Person
                background #08427b
                color #ffffff
            }
        }
    }
}
