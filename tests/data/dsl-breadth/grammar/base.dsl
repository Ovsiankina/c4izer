workspace "Base" "The workspace grammar.dsl extends (P2-16)." {
    model {
        legacy = softwareSystem "Legacy" "Holds old orders."
    }
    views {
        systemContext legacy "legacy" "Legacy context" {
            include *
        }
    }
}
