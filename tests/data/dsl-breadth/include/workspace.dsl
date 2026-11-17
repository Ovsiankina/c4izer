workspace "Included" {
    model {
        !include people.dsl
        !include systems
        user -> shop "Buys from"
        shop -> mail "Sends mail with"
    }
    views {
        systemContext shop "c1" {
            include *
        }
    }
}
