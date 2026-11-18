workspace "Dynamic" "Phase-3 fixture of the dynamic-view row (SRS 4.7): the probe views seq, comp and star." {
    model {
        user = person "User" "Uses the system."
        s = softwareSystem "System" "The system in focus." {
            a = container "App" "Handles requests." "Rust" {
                h = component "Handler" "Takes a request." "Rust"
                v = component "Validator" "Checks input." "Rust"
            }
            b = container "Store" "Keeps records." "SQL"
        }
        t = softwareSystem "Third Party" "Answers lookups." "External"
        user -> a "Sends requests to" "HTTPS"
        a -> b "Reads from and writes to" "SQL"
        a -> t "Looks up data in" "HTTPS"
        b -> a "Notifies of changes" "SQL"
        user -> h "Submits forms to" "HTTPS"
        h -> v "Validates input with"
        v -> b "Checks records in" "SQL"
    }
    views {
        dynamic s "seq" "A request with parallel lookups" {
            user -> a "Sends a request"
            {
                {
                    a -> b "Reads the record"
                }
                {
                    a -> t "Looks up the rate"
                    a -> b "Stores the rate"
                }
            }
            b -> a "Notifies of the change"
            a -> t "Confirms"
        }
        dynamic a "comp" "Form submission inside the app" {
            user -> h "Submits a form"
            h -> v "Validates the form"
            v -> b "Checks the record"
        }
        dynamic * "star" "The system in its estate" {
            user -> s "Sends a request"
            s -> t "Looks up data"
        }
    }
}
