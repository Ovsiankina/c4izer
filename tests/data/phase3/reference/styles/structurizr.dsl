// styles-themes fixture (P2-436, P2-438): every element style property,
// every shape, every relationship style property, light/dark blocks, a
// bundled and a local theme (with a theme icon), a local icon file,
// colour names, `colour` and `dashed`.
// Roles only, no personal names.
workspace "Styles" "Every style construct of the styles-themes row." {

    model {
        operator = person "Operator" "Runs the store day to day." "Staff"
        store = softwareSystem "Store" "Sells goods online." {
            c0 = container "Box part" "Part drawn as Box." "Rust" "S0"
            c1 = container "RoundedBox part" "Part drawn as RoundedBox." "Rust" "S1"
            c2 = container "Circle part" "Part drawn as Circle." "Rust" "S2"
            c3 = container "Ellipse part" "Part drawn as Ellipse." "Rust" "S3"
            c4 = container "Hexagon part" "Part drawn as Hexagon." "Rust" "S4"
            c5 = container "Diamond part" "Part drawn as Diamond." "Rust" "S5"
            c6 = container "Cylinder part" "Part drawn as Cylinder." "Rust" "S6"
            c7 = container "Bucket part" "Part drawn as Bucket." "Rust" "S7"
            c8 = container "Pipe part" "Part drawn as Pipe." "Rust" "S8"
            c9 = container "Person part" "Part drawn as Person." "Rust" "S9"
            c10 = container "Robot part" "Part drawn as Robot." "Rust" "S10"
            c11 = container "Folder part" "Part drawn as Folder." "Rust" "S11"
            c12 = container "WebBrowser part" "Part drawn as WebBrowser." "Rust" "S12"
            c13 = container "Window part" "Part drawn as Window." "Rust" "S13"
            c14 = container "Terminal part" "Part drawn as Terminal." "Rust" "S14"
            c15 = container "Shell part" "Part drawn as Shell." "Rust" "S15"
            c16 = container "MobileDevicePortrait part" "Part drawn as MobileDevicePortrait." "Rust" "S16"
            c17 = container "MobileDeviceLandscape part" "Part drawn as MobileDeviceLandscape." "Rust" "S17"
            c18 = container "Component part" "Part drawn as Component." "Rust" "S18"
        }
        partner = softwareSystem "Partner" "Takes the reports." "External"
        operator -> c0 "Runs the store through" "HTTPS" "Async"
        c0 -> c1 "Sends work to" "" "Async,Batch"
        c1 -> c0 "Answers"
        c1 -> c2 "Sends work to" "" "Sync"
        c2 -> c1 "Answers"
        c2 -> c3 "Sends work to"
        c3 -> c2 "Answers"
        c3 -> c4 "Sends work to" "" "Async,Batch"
        c4 -> c3 "Answers"
        c4 -> c5 "Sends work to" "" "Sync"
        c5 -> c4 "Answers"
        c5 -> c6 "Sends work to"
        c6 -> c5 "Answers"
        c6 -> c7 "Sends work to" "" "Async,Batch"
        c7 -> c6 "Answers"
        c7 -> c8 "Sends work to" "" "Sync"
        c8 -> c7 "Answers"
        c8 -> c9 "Sends work to"
        c9 -> c8 "Answers"
        c9 -> c10 "Sends work to" "" "Async,Batch"
        c10 -> c9 "Answers"
        c10 -> c11 "Sends work to" "" "Sync"
        c11 -> c10 "Answers"
        c11 -> c12 "Sends work to"
        c12 -> c11 "Answers"
        c12 -> c13 "Sends work to" "" "Async,Batch"
        c13 -> c12 "Answers"
        c13 -> c14 "Sends work to" "" "Sync"
        c14 -> c13 "Answers"
        c14 -> c15 "Sends work to"
        c15 -> c14 "Answers"
        c15 -> c16 "Sends work to" "" "Async,Batch"
        c16 -> c15 "Answers"
        c16 -> c17 "Sends work to" "" "Sync"
        c17 -> c16 "Answers"
        c17 -> c18 "Sends work to"
        c18 -> c17 "Answers"
        c18 -> c0 "Sends work to" "" "Async,Batch"
        c0 -> c18 "Answers"
        c5 -> partner "Reports to" "HTTPS" "Sync"
    }

    views {
        container store "all" "Every shape" {
            include *
        }

        systemContext store "ctx" "Store context" {
            include *
        }


        styles {
            element "S0" {
                shape Box
            }
            element "S1" {
                shape RoundedBox
            }
            element "S2" {
                shape Circle
            }
            element "S3" {
                shape Ellipse
            }
            element "S4" {
                shape Hexagon
            }
            element "S5" {
                shape Diamond
            }
            element "S6" {
                shape Cylinder
            }
            element "S7" {
                shape Bucket
            }
            element "S8" {
                shape Pipe
            }
            element "S9" {
                shape Person
            }
            element "S10" {
                shape Robot
            }
            element "S11" {
                shape Folder
            }
            element "S12" {
                shape WebBrowser
            }
            element "S13" {
                shape Window
            }
            element "S14" {
                shape Terminal
            }
            element "S15" {
                shape Shell
            }
            element "S16" {
                shape MobileDevicePortrait
            }
            element "S17" {
                shape MobileDeviceLandscape
            }
            element "S18" {
                shape Component
            }
            element "Staff" {
                shape Person
                icon data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==
                width 300
                height 260
                background SteelBlue
                colour white
                stroke #123456
                strokeWidth 2
                fontSize 16
                border dotted
                opacity 90
                metadata true
                description true
                properties {
                    owner operations
                }
            }
            element "S6" {
                icon icons/database.svg
                background #00796b
                color #ffffff
                border dashed
                metadata false
                description false
            }
            element "Boundary" {
                background #f4f4f4
                stroke Gray
                strokeWidth 2
                color #333333
                fontSize 14
                border dashed
                opacity 80
                metadata true
            }
            element "Boundary:SoftwareSystem" {
                stroke #1168bd
            }
            relationship "Async" {
                thickness 3
                colour DarkRed
                style dashed
                routing Direct
                jump true
                fontSize 12
                width 120
                position 40
                opacity 80
                properties {
                    kind async
                }
            }
            relationship "Batch" {
                dashed true
            }
            relationship "Sync" {
                style dotted
                routing Curved
                jump false
            }
            light {
                element "External" {
                    background #999999
                }
                relationship "Relationship" {
                    color #707070
                }
            }
            dark {
                element "External" {
                    background #444444
                }
                relationship "Relationship" {
                    colour #aaaaaa
                }
            }
        }
    }
}
