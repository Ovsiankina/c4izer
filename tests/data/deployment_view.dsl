workspace "Online shop" "Where an online shop runs." {

    model {
        customer = person "Customer" "Buys goods."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves the pages." "Rust"
            api = container "Api" "Takes orders." "Rust"
            db = container "Db" "Stores orders." "PostgreSQL" "Database"
        }
        mail = softwareSystem "Mail" "Sends mail." "External"

        customer -> web "Browses" "HTTPS"
        web -> api "Sends orders to" "JSON/HTTPS"
        api -> db "Reads and writes" "SQL"
        api -> mail "Sends mail through" "SMTP"

        live = deploymentEnvironment "Live" {
            blue = deploymentGroup "Blue"
            green = deploymentGroup "Green"
            cloud = deploymentNode "Cloud" "The cloud account." "AWS" {
                dns = infrastructureNode "DNS" "Routes requests." "Route 53"
                lb = infrastructureNode "Load balancer" "Spreads requests." "ELB"
                webHost = deploymentNode "Web host" "Runs the web tier." "EC2" {
                    instances 2
                    webBlue = containerInstance web "blue"
                    webGreen = containerInstance web "green"
                }
                apiHost = deploymentNode "Api host" "Runs the api." "EC2" {
                    instances 2..4
                    apiBlue = containerInstance api "blue"
                    apiGreen = containerInstance api "green"
                }
                database = deploymentNode "Database" "Managed database." "RDS" {
                    dbPrimary = containerInstance db "blue,green"
                }
                spare = deploymentNode "Spare" "Holds nothing yet." "EC2"
            }
            provider = deploymentNode "Mail provider" "A third party." "SaaS" {
                mailInstance = softwareSystemInstance mail "blue,green"
            }
            dns -> lb "Forwards requests to" "HTTPS"
            lb -> webBlue "Forwards requests to" "HTTPS"
            lb -> webGreen "Forwards requests to" "HTTPS"
            apiGreen -/> mailInstance
        }

        staging = deploymentEnvironment "Staging" {
            box = deploymentNode "Test box" "One machine." "Docker" {
                containerInstance web
                containerInstance api
                containerInstance db
            }
        }
    }

    views {
        deployment shop "Live" "live" {
            include *
            title "Shop in production"
            subtitle "Blue and green tiers"
        }
        deployment * "Staging" "staging" {
            include *
            title "Shop in staging"
        }
        filtered "live" exclude "Infrastructure Node" "live-apps" "Production without infrastructure"
        styles {
            element "Database" {
                shape cylinder
            }
            element "External" {
                background #999999
                color #ffffff
            }
            element "Infrastructure Node" {
                background #e8f0e0
            }
        }
    }
}
