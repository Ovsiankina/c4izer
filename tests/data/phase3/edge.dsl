// exporters fix-round fixture (SRS 4.7 P3-406..P3-412): groups, every
// shape, borders and lines with stroke widths, a large font, a URL, an
// icon, quotes, angle brackets, ampersands and non-ASCII text, a long
// name, an empty description, a response step and relationships to and
// from deployment nodes. Roles only, no personal names.
workspace "Edge \"cases\" & <more>" "Every corner the exporters must get right." {

    model {
        properties {
            "structurizr.groupSeparator" "/"
        }
        user = person "User \"Q\" & <Co>" "Uses things: \"quotes\" & <angles>, café au lait."
        group "Partners" {
            partner = softwareSystem "Partner" "" "External"
        }
        robot = softwareSystem "Robot" "Pokes things now and then." "Robo"
        shop = softwareSystem "Shop with a rather long name for wrapping tests" "A system whose description is long enough to wrap in several places, really." {
            url "https://example.com/shop?a=1&b=2"
            group "Front" {
                group "Edge" {
                    web = container "Web" "" "Rust" "Rounded"
                }
                api = container "Api \"v2\"" "Serves <things> & more." "Rust" "Hex"
            }
            db = container "Db" "Stores things." "SQL" "Circ"
        }
        shapes = softwareSystem "Shapes" "Every shape." {
            k0 = container "Box part" "Drawn as Box." "Kind" "ShapeBox"
            k1 = container "Person part" "Drawn as Person." "Kind" "ShapePerson"
            k2 = container "Cylinder part" "Drawn as Cylinder." "Kind" "ShapeCylinder"
            k3 = container "RoundedBox part" "Drawn as RoundedBox." "Kind" "ShapeRoundedBox"
            k4 = container "Circle part" "Drawn as Circle." "Kind" "ShapeCircle"
            k5 = container "Ellipse part" "Drawn as Ellipse." "Kind" "ShapeEllipse"
            k6 = container "Hexagon part" "Drawn as Hexagon." "Kind" "ShapeHexagon"
            k7 = container "Diamond part" "Drawn as Diamond." "Kind" "ShapeDiamond"
            k8 = container "Bucket part" "Drawn as Bucket." "Kind" "ShapeBucket"
            k9 = container "Pipe part" "Drawn as Pipe." "Kind" "ShapePipe"
            k10 = container "Robot part" "Drawn as Robot." "Kind" "ShapeRobot"
            k11 = container "Folder part" "Drawn as Folder." "Kind" "ShapeFolder"
            k12 = container "WebBrowser part" "Drawn as WebBrowser." "Kind" "ShapeWebBrowser"
            k13 = container "Window part" "Drawn as Window." "Kind" "ShapeWindow"
            k14 = container "Terminal part" "Drawn as Terminal." "Kind" "ShapeTerminal"
            k15 = container "Shell part" "Drawn as Shell." "Kind" "ShapeShell"
            k16 = container "MobileDevicePortrait part" "Drawn as MobileDevicePortrait." "Kind" "ShapeMobileDevicePortrait"
            k17 = container "MobileDeviceLandscape part" "Drawn as MobileDeviceLandscape." "Kind" "ShapeMobileDeviceLandscape"
            k18 = container "Component part" "Drawn as Component." "Kind" "ShapeComponent"
        }
        user -> web "Browses \"fast\" & <well>" "HTTPS"
        web -> api "Calls" "" "Dotted"
        api -> db "Reads" "" "Thick"
        robot -> shop "Pokes"
        api -> partner "Reports to" "HTTPS" "Solid"

        live = deploymentEnvironment "Live" {
            server = deploymentNode "Server" "" "Linux" {
                webInstance = containerInstance web
                apiInstance = containerInstance api
            }
            disk = deploymentNode "Store" "Keeps backups." "Disk" {
                dbInstance = containerInstance db
            }
            edge = deploymentNode "Edge" {
                proxy = infrastructureNode "Proxy" "Forwards." "nginx"
            }
            proxy -> server "Forwards to" "HTTPS"
            server -> disk "Backs up to"
        }
    }

    views {
        systemLandscape "landscape" "Everything at once" {
            include *
        }
        container shop "containers" {
            include *
        }
        container shapes "shapes" "Every shape, no relationships" {
            include *
        }
        dynamic shop "flow" "With a response" {
            user -> web "Opens"
            web -> api "Calls"
            api -> web "Answers"
        }
        deployment * "Live" "live" {
            include *
        }
        styles {
            element "ShapeBox" {
                shape Box
            }
            element "ShapePerson" {
                shape Person
            }
            element "ShapeCylinder" {
                shape Cylinder
            }
            element "ShapeRoundedBox" {
                shape RoundedBox
            }
            element "ShapeCircle" {
                shape Circle
            }
            element "ShapeEllipse" {
                shape Ellipse
            }
            element "ShapeHexagon" {
                shape Hexagon
            }
            element "ShapeDiamond" {
                shape Diamond
            }
            element "ShapeBucket" {
                shape Bucket
            }
            element "ShapePipe" {
                shape Pipe
            }
            element "ShapeRobot" {
                shape Robot
            }
            element "ShapeFolder" {
                shape Folder
            }
            element "ShapeWebBrowser" {
                shape WebBrowser
            }
            element "ShapeWindow" {
                shape Window
            }
            element "ShapeTerminal" {
                shape Terminal
            }
            element "ShapeShell" {
                shape Shell
            }
            element "ShapeMobileDevicePortrait" {
                shape MobileDevicePortrait
            }
            element "ShapeMobileDeviceLandscape" {
                shape MobileDeviceLandscape
            }
            element "ShapeComponent" {
                shape Component
            }
            element "Rounded" {
                shape RoundedBox
            }
            element "Hex" {
                shape Hexagon
                border dashed
                strokeWidth 4
                fontSize 32
                properties {
                    plantuml.shadow true
                }
            }
            element "Circ" {
                shape Circle
                border dotted
                strokeWidth 3
            }
            element "Robo" {
                shape Robot
                icon data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==
            }
            element "External" {
                background #999999
                color #ffffff
            }
            element "Group" {
                color #aa0000
            }
            relationship "Dotted" {
                style dotted
                thickness 3
            }
            relationship "Thick" {
                style dashed
                thickness 4
            }
            relationship "Solid" {
                style solid
            }
        }
    }
}
