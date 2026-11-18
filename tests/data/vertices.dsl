workspace "Vertices" "Fixture of the vertices row (SRS 4.7 P3-4, P3-300..P3-318): lines bent through hand-added vertices." {
    model {
        customer = person "Customer" "Buys goods."
        admin = person "Admin" "Runs the shop."
        shop = softwareSystem "Shop" "Sells goods." {
            web = container "Web" "Serves pages." "Rust"
            api = container "Api" "Answers calls." "Rust"
            db = container "Db" "Stores orders." "SQL" "Database"
        }
        bank = softwareSystem "Bank" "Moves money." "External"
        customer -> web "Browses" "HTTPS"
        admin -> web "Manages" "HTTPS"
        web -> api "Prices baskets through" "JSON"
        web -> db "Reads orders" "SQL"
        api -> db "Writes orders" "SQL"
        web -> bank "Charges cards" "HTTPS"
    }
    views {
        container shop "containers" "Inside the shop" {
            include *
            at customer 200 100
            at admin 700 100
            at web 200 400
            at api 700 400
            at db 200 800
            at bank 1200 400
            via web -> db -100 400 -100 800
            via web -> bank 950 250
        }
        dynamic shop "checkout" "Checkout" {
            customer -> web "Opens the basket"
            web -> api "Prices the basket"
            api -> db "Saves the order"
            web -> db "Reads the order back"
            at customer 200 100
            at web 200 400
            at api 700 400
            at db 700 800
            via 4: web -> db 200 800
        }
        styles {
            element "Database" {
                shape Cylinder
            }
        }
    }
}
