//! `Edit::SetMaxCrossings` (CORE-17, owner request 2026-11-23: the
//! editor's Settings menu): the view's `maxCrossings` line added,
//! rewritten in place, removed, and undone byte for byte.

use c4rizer_core::edit::{self, Edit};
use c4rizer_core::model;

const PAIR: &str = r#"workspace {
    model {
        a = softwareSystem "Alpha" "First system."
        b = softwareSystem "Beta" "Second system."
        a -> b "Sends orders to"
    }
    views {
        systemContext a "pair" "Pair" {
            include a b
            at a 200 300
            at b 900 300
SETTING
        }
    }
}
"#;

fn set(src: &str, budget: Option<u32>) -> edit::Applied {
    edit::apply(
        &model::parse(src).expect("parses"),
        &[Edit::SetMaxCrossings {
            view: "pair".to_owned(),
            budget,
        }],
    )
    .expect("applies")
}

fn budget_of(src: &str) -> u32 {
    model::parse(src).expect("parses").model.view("pair").expect("view").options().max_crossings
}

#[test]
fn added_after_the_last_option_line_and_undone() {
    let src = PAIR.replace("SETTING", "            labelWidth 20");
    let done = set(&src, Some(3));
    assert!(done.text.contains("labelWidth 20\n            maxCrossings 3\n"), "{}", done.text);
    assert_eq!(budget_of(&done.text), 3);
    let back = edit::apply(&model::parse(&done.text).expect("parses"), &done.inverse).expect("applies");
    assert_eq!(back.text, src);
}

#[test]
fn added_after_the_include_line_without_options() {
    let src = PAIR.replace("SETTING\n", "");
    let done = set(&src, Some(2));
    assert!(done.text.contains("include a b\n            maxCrossings 2\n"), "{}", done.text);
}

#[test]
fn rewritten_in_place_then_removed() {
    let src = PAIR.replace("SETTING", "            maxCrossings 1");
    let more = set(&src, Some(4));
    assert_eq!(more.text, src.replace("maxCrossings 1", "maxCrossings 4"));
    let back = edit::apply(&model::parse(&more.text).expect("parses"), &more.inverse).expect("applies");
    assert_eq!(back.text, src);
    let gone = set(&src, None);
    assert_eq!(gone.text, PAIR.replace("SETTING\n", ""));
    assert_eq!(budget_of(&gone.text), 0);
}

#[test]
fn none_on_a_view_without_the_line_changes_nothing() {
    let src = PAIR.replace("SETTING\n", "");
    assert_eq!(set(&src, None).text, src);
}
