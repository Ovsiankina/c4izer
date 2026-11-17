workspace extends grammar/base.dsl {
    name "Grammar"
    description "Every keyword of the Structurizr DSL language reference, in each block it may sit in (P2-1)."
    !identifiers flat
    !docs grammar/docs
    !adrs grammar/adrs
    !const ORG "Example"
    !var TEAM "Platform"
    properties {
        "structurizr.dslEditor" false
    }
    !script groovy {
        workspace.description = "Scripted { not DSL"
    }
    !plugin com.example.Plugin {
        key value
    }

    model {
        !identifiers flat
        properties {
            "structurizr.groupSeparator" "/"
        }
        !impliedRelationships true
        archetypes {
            service = container {
                technology "Go"
                tags "Service"
            }
            sync = -> {
                tags "Synchronous"
            }
        }
        !include grammar/people.dsl
        group "${ORG} partners" {
            bank = softwareSystem "Bank" "Takes payments." "External"
            group "Couriers" {
                courier = softwareSystem "Courier" "Delivers parcels." "External"
            }
        }
        scanner = element "Scanner" "Device" "Scans barcodes." {
            tag "Kit"
        }
        shop = softwareSystem "Shop" "Sells goods." {
            description "Sells goods online."
            tags "Internal"
            tag "Core"
            url https://example.com/shop
            properties {
                owner "${TEAM}"
            }
            perspectives {
                Security "Handles card data." "High"
            }
            !docs grammar/docs
            !adrs grammar/adrs
            !decisions grammar/adrs
            group "Front" {
                web = container "Web" "Serves pages." "Rust" {
                    technology "Rust 2024"
                    tags "Web"
                    tag "Public"
                    url https://example.com/web
                    properties {
                        tier front
                    }
                    perspectives {
                        Ops "Run by ops."
                    }
                    !docs grammar/docs
                    !adrs grammar/adrs
                    !components {
                        classes web.jar
                    }
                    group "Rendering" {
                        pages = component "Pages" "Renders pages." "Rust"
                    }
                    session = component "Session" "Keeps sessions." "Rust" {
                        group "State"
                        technology "Rust module"
                        description "Keeps the session."
                        tags "Stateful"
                        tag "Memory"
                        url https://example.com/session
                        properties {
                            ttl 30m
                        }
                        perspectives {
                            Ops "Restart drops sessions."
                        }
                        !docs grammar/docs
                        !adrs grammar/adrs
                        -> pages "Feeds" \
                            "In process"
                    }
                    pages -> api "Fetches orders from" "HTTPS"
                }
            }
            api = service "Api" "Takes orders." {
                -> bank "Charges cards with" "HTTPS"
            }
            db = container "Db" """
                Stores orders.
                """ "Postgres" "Database"
            api --sync-> db "Stores orders in"
        }
        customer -> web "Browses" "HTTPS"
        clerk -> shop "Answers questions with"
        uses = customer -> shop "Buys from"
        shop -> legacy "Reads old orders from"
        scanner -> api "Sends scans to"
        api -> courier "Books parcels with"
        live = deploymentEnvironment "Live" {
            edge = deploymentGroup "Edge"
            group "Region" {
                node = deploymentNode "Node" "A host." "Linux" {
                    instances 2
                    lb = infrastructureNode "Load balancer" "Spreads load." "Nginx"
                    webInstance = containerInstance web edge {
                        healthCheck "Web up" https://example.com/health 60 1000
                    }
                    softwareSystemInstance bank
                }
            }
            lb -> webInstance "Forwards to"
            lb -/> webInstance "Never calls"
        }
        !element shop {
            tag "Elemented"
        }
        !elements "element.tag==Core" {
            tag "Selected"
        }
        !relationship uses {
            tag "Directive"
        }
        !relationships "*->shop" {
            tag "Into shop"
        }
        !extend web {
            tag "Extended"
        }
        !ref uses {
            tag "Referenced"
        }
    }

    views {
        properties {
            "structurizr.groups" true
        }
        terminology {
            person "Actor"
        }
        systemLandscape "landscape" "Everything" {
            include *
            autoLayout
        }
        systemContext shop "context" "Shop context" {
            include *
            exclude scanner
            autoLayout lr
            default
            animation {
                customer
                shop
            }
            title "Shop context"
            description "The shop and its users."
            properties {
                "structurizr.tooltips" true
            }
        }
        container shop "containers" "Inside the shop" {
            include *
        }
        component web "components" "Inside the web app" {
            include *
        }
        filtered "landscape" include "Person" "people" "People only"
        dynamic shop "flow" "Checkout" {
            customer -> web "Opens the shop"
            {
                web -> api "Places the order"
            }
            uses "Buys"
        }
        deployment * live "live" "Live" {
            include *
            autoLayout
        }
        custom "board" "Whiteboard" {
            include customer shop
        }
        image * "picture" {
            image grammar/docs/01-shop.md
        }
        styles {
            element "Person" {
                shape Person
            }
            relationship "Relationship" {
                dashed false
            }
            light {
                element "Element" {
                    background #ffffff
                }
            }
            dark {
                element "Element" {
                    background #000000
                }
            }
        }
        theme default
        themes https://example.com/a.json https://example.com/b.json
    }

    configuration {
        scope softwaresystem
        visibility public
        users {
            someone write
        }
        properties {
            key value
        }
    }
}
