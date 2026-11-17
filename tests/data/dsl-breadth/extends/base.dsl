workspace "Base" "The shared model." {
    model {
        user = person "User"
        shop = softwareSystem "Shop" {
            web = container "Web"
        }
        user -> web "Browses"
    }
    views {
        systemContext shop "base-c1" {
            include *
        }
    }
}
