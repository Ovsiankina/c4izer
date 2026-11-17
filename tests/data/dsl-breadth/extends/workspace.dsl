workspace extends base.dsl {
    model {
        mail = softwareSystem "Mail" "" "External"
        web -> mail "Sends mail with"
        !element shop {
            tags "Extended"
        }
    }
    views {
        container shop "c2" {
            include *
        }
    }
}
