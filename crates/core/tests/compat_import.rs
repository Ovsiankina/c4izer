//! Tests of `import::import_layout` (SRS 4.6: P2-15, P2-504..P2-511).
//! One `#[test]` per requirement, named after it. Hand-made data uses
//! roles, never personal names (SRS 5.4).
//!
//! The JSON is the shape Structurizr writes (`structurizr-core`
//! `Workspace` serialised): `model.people`, `model.softwareSystems[]
//! .containers[].components[]`, `views.<kind>Views[].elements[]` with
//! `id`, `x`, `y`, and `views.configuration.styles.elements[]`.

use c4rizer_core::edit::{self, Edit};
use c4rizer_core::import::{self, Import};
use c4rizer_core::model::{self, FsLoader, NoticeKind, Parsed};

/// The file under test: a bank, its containers, an external mail
/// system, and views of each drawable kind.
const DSL: &str = r#"workspace "Bank" {
    model {
        user = person "Customer" "A customer of the bank."
        sys = softwareSystem "Banking System" "Handles accounts." {
            web = container "Web App" "Serves pages." "Rust"
            db = container "Database" "Stores accounts." "SQL"
        }
        ext = softwareSystem "Mail System" "Sends e-mail." "External"
        board = element "Board" "Whiteboard" "Holds the plan."
        note = element "Note" "Paper" "One task."
        user -> web "Uses"
        web -> db "Reads"
        web -> ext "Sends via"
    }
    views {
        systemContext sys "Context" "The bank" {
            include *
        }
        container sys "Containers" "Inside the bank" {
            include *
        }
        custom "Custom" "Two boxes" {
            include board note
        }
        filtered "Context" include "Element,Relationship" "Filtered" "All of it"
    }
}
"#;

/// The JSON model of `DSL` with Structurizr-style ids.
const MODEL: &str = r#""model": {
    "people": [{"id": "1", "name": "Customer", "description": "A customer of the bank.", "tags": "Element,Person"}],
    "softwareSystems": [
        {"id": "2", "name": "Banking System", "description": "Handles accounts.", "tags": "Element,Software System",
         "containers": [
            {"id": "3", "name": "Web App", "description": "Serves pages.", "tags": "Element,Container"},
            {"id": "4", "name": "Database", "description": "Stores accounts.", "tags": "Element,Container"}
         ]},
        {"id": "5", "name": "Mail System", "description": "Sends e-mail.", "tags": "Element,Software System,External"}
    ],
    "customElements": [
        {"id": "10", "name": "Board", "metadata": "Whiteboard", "description": "Holds the plan.", "tags": "Element"},
        {"id": "11", "name": "Note", "metadata": "Paper", "description": "One task.", "tags": "Element"}
    ]
}"#;

/// Styles giving every box 400x200, so centres are easy to read.
const STYLES: &str = r#""configuration": {"styles": {"elements": [{"tag": "Element", "width": 400, "height": 200}]}}"#;

/// A workspace JSON with `model` and the `views` members `views`.
fn ws(model: &str, views: &str) -> String {
    format!("{{\"id\": 1, \"name\": \"Bank\", {model}, \"views\": {{{views}}}}}")
}

/// One JSON view of `array` with key `key` and element positions.
fn view(array: &str, key: &str, elements: &[(&str, i32, i32)]) -> String {
    let els: Vec<String> =
        elements.iter().map(|(id, x, y)| format!("{{\"id\": \"{id}\", \"x\": {x}, \"y\": {y}}}")).collect();
    format!("\"{array}\": [{{\"key\": \"{key}\", \"elements\": [{}]}}]", els.join(", "))
}

fn parse(src: &str) -> Parsed {
    model::parse(src).unwrap_or_else(|e| panic!("{e}"))
}

fn run(src: &str, json: &str) -> Import {
    import::import_layout(&parse(src), json).unwrap_or_else(|e| panic!("{e}"))
}

/// The `(view, element, centre)` of every edit.
fn moves(import: &Import) -> Vec<(String, String, (i32, i32))> {
    import
        .edits
        .iter()
        .map(|e| match e {
            Edit::Move { view, element, centre: Some(c) } => (view.clone(), element.clone(), *c),
            other => panic!("import writes only `at` moves, got {other:?}"),
        })
        .collect()
}

/// `(kind, subject, view)` of every notice.
fn notices(import: &Import) -> Vec<(NoticeKind, String, Option<String>)> {
    import.notices.iter().map(|n| (n.kind, n.subject.clone(), n.view.clone())).collect()
}

fn has_notice(import: &Import, kind: NoticeKind, subject: &str) -> bool {
    import.notices.iter().any(|n| n.kind == kind && n.subject == subject)
}

/// The file after applying the batch.
fn applied(src: &str, import: &Import) -> String {
    edit::apply(&parse(src), &import.edits).unwrap_or_else(|e| panic!("{e:?}")).text
}

/// The context view's three boxes at simple top-left positions.
fn context_json() -> String {
    ws(
        MODEL,
        &format!("{}, {STYLES}", view("systemContextViews", "Context", &[("1", 0, 0), ("2", 400, 400), ("5", 800, 800)])),
    )
}

// ---- P2-504 -----------------------------------------------------------

#[test]
fn p2_504_not_a_workspace_is_error() {
    for json in ["{\"model\": {}}", "[]", "{\"views\": []}"] {
        let err = import::import_layout(&parse(DSL), json).expect_err("not a workspace");
        assert_eq!(err.position, None, "{json}");
        assert!(err.to_string().contains("views"), "{err}");
    }
}

// ---- P2-505 -----------------------------------------------------------

#[test]
fn p2_505_views_matched_by_key() {
    let import = run(DSL, &context_json());
    assert_eq!(
        moves(&import),
        vec![
            ("Context".to_owned(), "user".to_owned(), (200, 100)),
            ("Context".to_owned(), "sys".to_owned(), (600, 500)),
            ("Context".to_owned(), "ext".to_owned(), (1000, 900)),
        ]
    );
    assert_eq!(import.views.len(), 1);
    let v = &import.views[0];
    assert_eq!((v.key.as_str(), v.positions, v.generated_key), ("Context", 3, false));
    assert!(import.notices.is_empty(), "{:?}", notices(&import));
}

#[test]
fn p2_505_custom_view_matched() {
    let json = ws(MODEL, &format!("{}, {STYLES}", view("customViews", "Custom", &[("10", 0, 0), ("11", 0, 400)])));
    let import = run(DSL, &json);
    assert_eq!(
        moves(&import),
        vec![("Custom".to_owned(), "board".to_owned(), (200, 100)), ("Custom".to_owned(), "note".to_owned(), (200, 500))]
    );
}

#[test]
fn p2_505_json_view_without_dsl_view_skipped() {
    let json = ws(MODEL, &format!("{}, {STYLES}", view("systemContextViews", "Elsewhere", &[("1", 0, 0)])));
    let import = run(DSL, &json);
    assert!(import.edits.is_empty());
    assert!(import.views.is_empty());
    assert_eq!(
        notices(&import),
        vec![(NoticeKind::ViewNotInFile, "Elsewhere".to_owned(), Some("Elsewhere".to_owned()))]
    );
}

#[test]
fn p2_505_dsl_view_without_json_view_untouched() {
    let import = run(DSL, &context_json());
    assert!(moves(&import).iter().all(|(v, _, _)| v == "Context"));
    let after = applied(DSL, &import);
    let block =
        |text: &str| text[text.find("container sys").expect("view")..].split('}').next().unwrap_or("").to_owned();
    assert_eq!(block(&after), block(DSL), "the Containers view is untouched");
}

#[test]
fn p2_505_generated_key_match_warns() {
    let src = DSL
        .replace("systemContext sys \"Context\" \"The bank\" {", "systemContext sys {")
        .replace("        filtered \"Context\" include \"Element,Relationship\" \"Filtered\" \"All of it\"\n", "");
    let parsed = parse(&src);
    let key = parsed.model.views[0].key.clone();
    assert!(parsed.model.views[0].declared_key.is_none());
    let json = ws(MODEL, &format!("{}, {STYLES}", view("systemContextViews", &key, &[("1", 0, 0)])));
    let import = run(&src, &json);
    assert!(import.views[0].generated_key);
    assert!(has_notice(&import, NoticeKind::GeneratedKey, &key), "{:?}", notices(&import));
    assert_eq!(moves(&import)[0].1, "user", "the match still imports");
}

// ---- P2-506 -----------------------------------------------------------

/// `MODEL` with a second system, listed first, that also holds a "Web
/// App" container.
fn model_with_twin(web_name: &str, system_name: &str, web_description: &str) -> String {
    format!(
        r#""model": {{
    "people": [{{"id": "1", "name": "Customer", "description": "A customer of the bank.", "tags": "Element,Person"}}],
    "softwareSystems": [
        {{"id": "20", "name": "Other System", "tags": "Element,Software System",
         "containers": [{{"id": "21", "name": "Web App", "description": "Serves pages.", "tags": "Element,Container"}}]}},
        {{"id": "2", "name": "{system_name}", "description": "Handles accounts.", "tags": "Element,Software System",
         "containers": [
            {{"id": "3", "name": "{web_name}", "description": "{web_description}", "tags": "Element,Container"}},
            {{"id": "4", "name": "Database", "description": "Stores accounts.", "tags": "Element,Container"}}
         ]}},
        {{"id": "5", "name": "Mail System", "description": "Sends e-mail.", "tags": "Element,Software System,External"}}
    ]
}}"#
    )
}

/// The centre the containers view gives `web`.
fn web_centre(model: &str, elements: &[(&str, i32, i32)]) -> Option<(i32, i32)> {
    let json = ws(model, &format!("{}, {STYLES}", view("containerViews", "Containers", elements)));
    moves(&run(DSL, &json)).into_iter().find(|(_, e, _)| e == "web").map(|(_, _, c)| c)
}

#[test]
fn p2_506_canonical_name_wins() {
    // The twin comes first and has the same type and name; the canonical
    // name (`Container://Banking System.Web App`) picks the second.
    let model = model_with_twin("Web App", "Banking System", "Serves pages.");
    assert_eq!(web_centre(&model, &[("21", 0, 0), ("3", 1000, 1000)]), Some((1200, 1100)));
}

#[test]
fn p2_506_falls_back_to_type_and_name() {
    // The system was renamed, so no canonical name matches; the first
    // container named "Web App" in the view wins.
    let model = model_with_twin("Web App", "Old Bank", "Serves pages.");
    assert_eq!(web_centre(&model, &[("3", 1000, 1000), ("21", 0, 0)]), Some((1200, 1100)));
    // A person named "Web App" is the wrong type.
    let json = ws(
        r#""model": {"people": [{"id": "9", "name": "Web App", "tags": "Element,Person"}]}"#,
        &format!("{}, {STYLES}", view("containerViews", "Containers", &[("9", 0, 0)])),
    );
    assert!(moves(&run(DSL, &json)).iter().all(|(_, e, _)| e != "web"));
}

#[test]
fn p2_506_falls_back_to_type_and_description() {
    // Renamed container and system, same description. (The twin is left
    // out of the view: its name "Web App" would match at step 2.)
    let model = model_with_twin("Web Application", "Old Bank", "Serves pages.");
    assert_eq!(web_centre(&model, &[("4", 0, 0), ("3", 1000, 1000)]), Some((1200, 1100)));
}

#[test]
fn p2_506_empty_description_never_matches() {
    let src = DSL.replace("container \"Web App\" \"Serves pages.\" \"Rust\"", "container \"Web App\" \"\" \"Rust\"");
    let model = model_with_twin("Web Application", "Old Bank", "");
    let json = ws(&model, &format!("{}, {STYLES}", view("containerViews", "Containers", &[("3", 0, 0)])));
    let import = run(&src, &json);
    assert!(moves(&import).iter().all(|(_, e, _)| e != "web"));
    assert!(has_notice(&import, NoticeKind::UnmatchedBox, "Web App"), "{:?}", notices(&import));
}

#[test]
fn p2_506_unknown_json_element_id_skipped() {
    let json =
        ws(MODEL, &format!("{}, {STYLES}", view("systemContextViews", "Context", &[("99", 0, 0), ("1", 0, 0)])));
    let import = run(DSL, &json);
    assert_eq!(notices(&import)[0], (NoticeKind::UnknownJsonElement, "99".to_owned(), Some("Context".to_owned())));
    assert!(moves(&import).iter().any(|(_, e, _)| e == "user"), "the rest still imports");
}

// ---- P2-507 -----------------------------------------------------------

#[test]
fn p2_507_json_only_element_not_imported() {
    // The database is in the JSON view but not drawn in the context view.
    let elements = [("1", 0, 0), ("2", 0, 0), ("5", 0, 0), ("4", 0, 0)];
    let json = ws(MODEL, &format!("{}, {STYLES}", view("systemContextViews", "Context", &elements)));
    let import = run(DSL, &json);
    let moved: Vec<String> = moves(&import).into_iter().map(|(_, e, _)| e).collect();
    assert_eq!(moved, ["user", "sys", "ext"]);
    assert!(import.notices.is_empty(), "{:?}", notices(&import));
}

#[test]
fn p2_507_unmatched_box_warned() {
    let src = DSL.replace(
        "\"The bank\" {\n            include *\n",
        "\"The bank\" {\n            include *\n            at ext 1500 1500\n",
    );
    let json = ws(MODEL, &format!("{}, {STYLES}", view("systemContextViews", "Context", &[("1", 0, 0), ("2", 0, 0)])));
    let import = run(&src, &json);
    assert_eq!(
        notices(&import),
        vec![(NoticeKind::UnmatchedBox, "Mail System".to_owned(), Some("Context".to_owned()))]
    );
    assert!(applied(&src, &import).contains("at ext 1500 1500"), "an unmatched box keeps its `at` line");
    let text = import.notices[0].to_string();
    assert!(text.contains("Context") && text.contains("Mail System"), "{text}");
}

// ---- P2-511 -----------------------------------------------------------

#[test]
fn p2_511_box_without_identifier_warned_not_edited() {
    let src = DSL
        .replace(
            "        board = element",
            "        element \"Ledger\" \"Book\" \"Keeps the books.\"\n        board = element",
        )
        .replace("include board note", "include *");
    let model = MODEL.replace(
        "{\"id\": \"10\"",
        "{\"id\": \"6\", \"name\": \"Ledger\", \"description\": \"Keeps the books.\", \"tags\": \"Element\"},\n        {\"id\": \"10\"",
    );
    let elements = [("10", 0, 0), ("6", 0, 0), ("11", 0, 0)];
    let json = ws(&model, &format!("{}, {STYLES}", view("customViews", "Custom", &elements)));
    let import = run(&src, &json);
    assert!(has_notice(&import, NoticeKind::NoIdentifier, "Ledger"), "{:?}", notices(&import));
    assert_eq!(moves(&import).len(), import.views[0].positions as usize);
    assert!(moves(&import).iter().all(|(_, e, _)| e != "Ledger"));
    applied(&src, &import); // `edit::apply` accepts the whole batch
}

// ---- P2-508 -----------------------------------------------------------

/// The centre of `user` in the context view with the given styles.
fn user_centre(styles: &str, x: i32, y: i32) -> (i32, i32) {
    let json = ws(
        MODEL,
        &format!(
            "{}, \"configuration\": {{\"styles\": {{\"elements\": [{styles}]}}}}",
            view("systemContextViews", "Context", &[("1", x, y)])
        ),
    );
    moves(&run(DSL, &json)).into_iter().find(|(_, e, _)| e == "user").expect("user moved").2
}

#[test]
fn p2_508_centre_from_top_left_with_style_size() {
    assert_eq!(user_centre(r#"{"tag": "Person", "width": 300, "height": 200}"#, 100, 50), (250, 150));
}

#[test]
fn p2_508_later_tag_style_wins() {
    // Tags are `Element,Person`: Person comes later and wins where it sets
    // a size, whatever the order of the style list.
    let styles = r#"{"tag": "Person", "height": 200}, {"tag": "Element", "width": 500, "height": 100}"#;
    assert_eq!(user_centre(styles, 0, 0), (250, 100));
}

#[test]
fn p2_508_snapped_to_grid() {
    let style = r#"{"tag": "Element", "width": 300, "height": 200}"#;
    assert_eq!(user_centre(style, 103, 0).0, 250);
    assert_eq!(user_centre(style, 105, 0).0, 260);
    assert_eq!(user_centre(style, -105, 0).0, 50, "half rounds toward positive infinity");
}

#[test]
#[ignore = "deferred"]
fn p2_508_default_size() {
    // TODO(srs): P2-508 default size; 450x300 taken (Part B
    // compat-fixtures 2) until the owner confirms it.
    assert_eq!(user_centre(r##"{"tag": "Person", "color": "#ffffff"}"##, 0, 0), (230, 150));
}

// ---- P2-15, P2-509 ------------------------------------------------------

#[test]
fn p2_15_import_layout_writes_at_lines() {
    let import = run(DSL, &context_json());
    let after = applied(DSL, &import);
    let parsed = parse(&after);
    let context = parsed.model.view("Context").expect("view");
    let at: Vec<(String, i32, i32)> = context
        .positions
        .iter()
        .map(|a| (parsed.model.full_key(a.element.value).unwrap_or_default(), a.x.value, a.y.value))
        .collect();
    assert_eq!(at, [("user".to_owned(), 200, 100), ("sys".to_owned(), 600, 500), ("ext".to_owned(), 1000, 900)]);
    assert!(at.iter().all(|(_, x, y)| x % 10 == 0 && y % 10 == 0), "on the 10px grid");
}

/// The lines of `text` that are not `at` lines.
fn non_at_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| !l.trim_start().starts_with("at ")).collect()
}

/// `src` with `line` added after the context view's `include *`.
fn with_context_line(line: &str) -> String {
    DSL.replace("\"The bank\" {\n            include *\n", &format!("\"The bank\" {{\n            include *\n            {line}\n"))
}

#[test]
fn p2_509_only_at_lines_change() {
    let src = with_context_line("at user 0 0");
    let after = applied(&src, &run(&src, &context_json()));
    assert_eq!(non_at_lines(&after), non_at_lines(&src));
    assert_ne!(after, src);
}

#[test]
fn p2_509_one_line_view_split_only() {
    let src = DSL.replace(
        "systemContext sys \"Context\" \"The bank\" {\n            include *\n        }",
        "systemContext sys \"Context\" \"The bank\" { include * }",
    );
    let after = applied(&src, &run(&src, &context_json()));
    let words = |lines: Vec<&str>| lines.join(" ").split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(words(non_at_lines(&after)), words(non_at_lines(&src)), "only the DSL-9 split and `at` lines");
    assert_eq!(after.lines().filter(|l| l.trim_start().starts_with("at ")).count(), 3);
}

#[test]
fn p2_509_existing_at_overwritten() {
    let src = with_context_line("at user 1000 1000");
    let after = applied(&src, &run(&src, &context_json()));
    assert!(after.contains("at user 200 100") && !after.contains("at user 1000 1000"), "{after}");
}

#[test]
fn p2_509_include_writes_root_file() {
    let dir = std::env::temp_dir().join(format!("c4rizer-p2-509-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let model_start = DSL.find("    model {").expect("model");
    let views_start = DSL.find("    views {").expect("views");
    std::fs::write(dir.join("model.dsl"), &DSL[model_start..views_start]).expect("write");
    let root = format!("{}    !include model.dsl\n{}", &DSL[..model_start], &DSL[views_start..]);
    std::fs::write(dir.join("workspace.dsl"), &root).expect("write");
    let parsed = model::parse_with(&root, Some(&dir.join("workspace.dsl")), &mut FsLoader).expect("parses");
    let import = import::import_layout(&parsed, &context_json()).expect("imports");
    let done = edit::apply(&parsed, &import.edits).expect("applies");
    std::fs::remove_dir_all(&dir).ok();
    assert!(done.files.is_empty(), "the included model file is not touched");
    assert!(done.text.contains("at user 200 100"), "{}", done.text);
}

#[test]
fn p2_509_no_match_no_edit() {
    let json = ws(MODEL, &format!("{}, {STYLES}", view("componentViews", "Components", &[("1", 0, 0)])));
    let import = run(DSL, &json);
    assert!(import.edits.is_empty());
    assert_eq!(applied(DSL, &import), DSL, "nothing to write");
}

// ---- P2-510 -----------------------------------------------------------

#[test]
fn p2_510_vertices_routing_paper_size_themes_named() {
    let views = r#""systemContextViews": [{"key": "Context", "paperSize": "A4_Landscape", "dimensions": {"width": 100, "height": 100},
            "elements": [{"id": "1", "x": 0, "y": 0}],
            "relationships": [
                {"id": "7", "vertices": [{"x": 1, "y": 2}], "routing": "Orthogonal", "position": 70},
                {"id": "8", "vertices": [{"x": 3, "y": 4}]},
                {"id": "9", "vertices": []}
            ]}],
        "configuration": {"themes": ["https://example.invalid/theme.json"], "styles": {"elements": [{"tag": "Element", "width": 400, "height": 200}]}}"#;
    let import = run(DSL, &ws(MODEL, views));
    let not_imported: Vec<String> =
        import.notices.iter().filter(|n| n.kind == NoticeKind::NotImported).map(|n| n.subject.clone()).collect();
    assert_eq!(not_imported, ["themes", "paperSize", "dimensions", "vertices", "routing", "position"], "each kind once");
    assert!(moves(&import).iter().any(|(_, e, _)| e == "user"));
}

#[test]
fn p2_510_undrawable_view_skipped() {
    let views = format!("{}, {}", view("deploymentViews", "Live", &[]), view("imageViews", "Picture", &[]));
    let import = run(DSL, &ws(MODEL, &views));
    assert!(import.edits.is_empty());
    let skipped: Vec<(NoticeKind, String)> = import.notices.iter().map(|n| (n.kind, n.subject.clone())).collect();
    assert_eq!(
        skipped,
        [(NoticeKind::ViewNotDrawable, "Live".to_owned()), (NoticeKind::ViewNotDrawable, "Picture".to_owned())]
    );
}

/// A JSON dynamic view is matched by key like a static one, and each box
/// its plan draws (the step ends) takes its position (dynamic views draw
/// since the dynamic-view row, P3-200); a dynamic view key the file
/// lacks is skipped as not in the file.
#[test]
fn p2_510_dynamic_view_positions_imported() {
    let dsl = DSL.replace(
        "        filtered ",
        "        dynamic sys \"Flow\" {\n            user -> web\n            web -> db\n        }\n        filtered ",
    );
    let views = format!("{}, {STYLES}", view("dynamicViews", "Flow", &[("4", 0, 800), ("1", 0, 0), ("3", 0, 400)]));
    let import = run(&dsl, &ws(MODEL, &views));
    assert_eq!(
        moves(&import),
        [
            ("Flow".to_owned(), "user".to_owned(), (200, 100)),
            ("Flow".to_owned(), "web".to_owned(), (200, 500)),
            ("Flow".to_owned(), "db".to_owned(), (200, 900)),
        ]
    );
    assert!(import.notices.is_empty(), "{:?}", notices(&import));
    let after = parse(&applied(&dsl, &import));
    assert_eq!(after.model.view("Flow").expect("view").positions.len(), 3);
    let import = run(DSL, &ws(MODEL, &format!("{}, {STYLES}", view("dynamicViews", "Flow", &[("1", 0, 0)]))));
    assert!(import.edits.is_empty());
    assert!(has_notice(&import, NoticeKind::ViewNotInFile, "Flow"));
}

#[test]
fn p2_510_filtered_views_ignored() {
    let views = format!(
        "\"filteredViews\": [{{\"key\": \"Filtered\", \"baseViewKey\": \"Context\", \"mode\": \"Include\", \"tags\": [\"Element\"]}}], {STYLES}"
    );
    let import = run(DSL, &ws(MODEL, &views));
    assert!(import.edits.is_empty() && import.notices.is_empty() && import.views.is_empty());
    // A file filtered view with the key of a JSON view is not written to:
    // it takes its base view's `at` lines (P2-9).
    let json = ws(MODEL, &format!("{}, {STYLES}", view("systemContextViews", "Filtered", &[("1", 0, 0)])));
    let import = run(DSL, &json);
    assert!(import.edits.is_empty());
    assert!(has_notice(&import, NoticeKind::ViewNotDrawable, "Filtered"));
}
