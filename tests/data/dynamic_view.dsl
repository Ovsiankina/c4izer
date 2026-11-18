workspace "Shop" "Fixture of the dynamic-view row (SRS 4.7 P3-3, P3-200..P3-221): numbered steps, parallel blocks, the same relationship twice, scope boundaries." {
    model {
        customer = person "Customer" "Buys goods online."
        clerk = person "Clerk" "Packs and ships orders."
        shop = softwareSystem "Shop" "Sells goods online." {
            web = container "Web App" "Serves the shop pages." "Rust" {
                basket = component "Basket" "Holds the chosen goods." "Rust"
                checkout = component "Checkout" "Turns a basket into an order." "Rust"
                pricing = component "Pricing" "Works out totals and tax." "Rust"
            }
            db = container "Database" "Stores orders and stock." "PostgreSQL"
            queue = container "Queue" "Carries order events." "NATS"
        }
        bank = softwareSystem "Bank" "Takes card payments." "External"
        mail = softwareSystem "Mail Service" "Delivers e-mail." "External"

        customer -> web "Orders goods with" "HTTPS"
        web -> db "Reads and writes orders in" "SQL"
        db -> web "Returns rows to" "SQL"
        charge = web -> bank "Charges the card through" "HTTPS"
        web -> queue "Publishes order events to" "NATS"
        queue -> mail "Triggers mails through" "SMTP"
        mail -> customer "Sends confirmations to" "SMTP"
        clerk -> web "Checks open orders in" "HTTPS"
        customer -> basket "Adds goods to" "HTTPS"
        basket -> checkout "Hands the basket to"
        checkout -> pricing "Asks for the total from"
        pricing -> db "Looks up prices in" "SQL"
        checkout -> bank "Authorises the payment with" "HTTPS"
    }
    views {
        dynamic shop "checkout" "Placing an order" {
            customer -> web "Submits the order"
            web -> db "Loads the basket"
            db -> web "Returns the basket"
            {
                {
                    charge "Charges the card"
                }
                {
                    web -> queue "Publishes order placed"
                    queue -> mail "Asks for a confirmation"
                }
            }
            web -> db "Saves the order"
            mail -> customer "Sends the confirmation"
        }
        dynamic web "pricing" "Pricing a basket" {
            customer -> basket "Opens the basket"
            basket -> checkout "Starts the checkout"
            checkout -> pricing "Asks for the total"
            pricing -> db "Looks up prices"
            checkout -> bank "Authorises the payment"
        }
        dynamic * "estate" "Ordering across systems" {
            customer -> shop "Places an order"
            shop -> bank "Takes the payment"
            shop -> mail "Asks for a confirmation"
            mail -> customer "Delivers the confirmation"
        }
        dynamic shop "written" "Fulfilment with written numbers" {
            1: clerk -> web "Opens the order list"
            2: web -> db "Fetches open orders"
            2a: db -> web "Returns open orders"
            3: web -> queue "Marks the order shipped"
        }
    }
}
