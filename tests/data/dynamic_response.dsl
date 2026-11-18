workspace "Sign in" "Fixture of response steps in a dynamic view (SRS 4.7 P3-201): a step written against the only declared relationship, after a step that uses it, is a response, as in the corpus' big-bank-plc.dsl SignIn view." {
    model {
        user = person "User" "Signs in."
        bank = softwareSystem "Bank" "Holds accounts." {
            app = container "App" "Takes credentials." "Rust"
            db = container "Database" "Stores users." "PostgreSQL"
        }

        user -> app "Signs in with" "HTTPS"
        app -> db "Reads users from" "SQL"
    }
    views {
        dynamic bank "signin" "Signing in" {
            user -> app "Submits credentials"
            app -> db "Looks up the user"
            db -> app "Returns the user"
            app -> user "Sends back a token"
        }
    }
}
