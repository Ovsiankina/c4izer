//! Step-4 contract tests of the styles-themes row (SRS 4.6: P2-10..13,
//! P2-21, P2-27, P2-400..P2-440). One or more `#[test]` per requirement,
//! named after it; bodies are `todo!()` until step 5. Fixtures go under
//! `tests/data/styles/`. The CLI tests (P2-432) are in
//! `crates/cli/tests/styles_themes.rs`, the editor tests (P2-417 panel,
//! P2-433, P2-436 canvas) in `crates/editor/src/styles_themes_tests.rs`.
//!
//! Each `TODO(srs)` names the SRS hole and the reading taken (the
//! Structurizr-faithful one, from docs/plans/phase2-decisions.md Part B,
//! pending the owner's yes).

#![allow(clippy::todo)]

// ---- Element style properties (styles-themes-01)

/// Each of `shape`, `icon`, `width`, `height`, `background`, `color`,
/// `colour`, `stroke`, `strokeWidth`, `fontSize`, `border`, `opacity`,
/// `metadata`, `description` and `properties` in an `element` block (top-
/// level and in `light`/`dark`) lands in `ElementStyle` with its value
/// span.
#[test]
fn p2_10_every_element_property_parsed() {
    todo!()
}

/// Any other word in an `element` block is still a DSL-4 error naming it.
#[test]
fn p2_10_unknown_property_still_rejected() {
    todo!()
}

/// `colour #112233` sets the same property as `color #112233`.
#[test]
fn p2_400_colour_alias() {
    todo!()
}

/// A block setting both `color` and `colour` is the same duplicate-property
/// error as `color` twice.
#[test]
fn p2_400_color_and_colour_duplicate() {
    todo!()
}

/// A CSS/HTML colour name resolves to its CSS hex value for element
/// `background`, `color`/`colour` and `stroke`.
#[test]
fn p2_401_named_colours() {
    todo!()
}

/// `RED`, `Red` and `red` all resolve to #ff0000.
#[test]
fn p2_401_named_colour_any_case() {
    todo!()
}

/// A word that is neither `#rrggbb` nor a CSS colour name is an invalid-
/// value error.
#[test]
fn p2_401_unknown_colour_name_invalid() {
    todo!()
}

/// `width W` and `height H` land in `ResolvedStyle::width`/`height`, each
/// on its own.
#[test]
fn p2_402_width_height_resolved() {
    todo!()
}

/// A width that is not a multiple of 10 is rounded up to the next multiple
/// of 10, taken as literal px.
#[test]
fn p2_402_off_grid_width() {
    // TODO(srs): P2-402, off-grid sizes and scale. Reading taken (B styles-
    // themes 4): literal px, rounded up to the next multiple of 10.
    todo!()
}

/// `stroke <colour>` lands in `ElementStyle::stroke` and
/// `ResolvedStyle::stroke`.
#[test]
fn p2_403_stroke_parsed() {
    todo!()
}

/// `strokeWidth N` is kept for 1..10; a value outside the range is clamped.
#[test]
fn p2_404_stroke_width_range() {
    // TODO(srs): P2-404 says invalid-value error; B styles-themes 13 [I]
    // (Structurizr for Java) clamps with no error. Reading taken: clamp;
    // verify at the pinned commit.
    todo!()
}

/// `border solid|dashed|dotted` lands in `ElementStyle::border`; the
/// resolved default is `Border::Solid`.
#[test]
fn p2_405_border_parsed() {
    todo!()
}

/// `opacity N` is kept for 0..100; a value outside the range is clamped.
#[test]
fn p2_406_opacity_range() {
    // TODO(srs): P2-406 says invalid-value error; B styles-themes 13 [I]
    // clamps. Reading taken: clamp.
    todo!()
}

/// `metadata false` and `description false` land in the style; both resolve
/// `true` by default.
#[test]
fn p2_408_metadata_description_flags() {
    todo!()
}

/// `properties { name value }` in an element style is parsed into
/// `Property` values, prints back byte for byte and changes no scene.
#[test]
fn p2_410_element_style_properties_kept() {
    todo!()
}

// ---- Shapes (styles-themes-02, -08, -09)

/// All 19 reference shapes parse in any letter case.
#[test]
fn p2_11_all_shapes_parse_case_insensitive() {
    todo!()
}

/// An unknown shape word is an invalid-value error.
#[test]
fn p2_11_unknown_shape_invalid() {
    todo!()
}

/// `Shape::keyword` gives the reference spelling of every shape
/// (`MobileDevicePortrait`).
#[test]
fn p2_11_shape_keyword_reference_spelling() {
    todo!()
}

/// Each of the 19 shapes builds its own `Outline` variant (or `Ellipse {
/// circle }` / `Window { kind }` / `MobileDevice { portrait }` value).
#[test]
fn p2_11_each_shape_distinct_outline() {
    todo!()
}

/// For every shape the outline lies inside the box rectangle and every text
/// inside the outline's text area.
#[test]
fn p2_413_outline_inside_rect_texts_inside_outline() {
    todo!()
}

/// Each outline's proportions follow Structurizr's notation page.
#[test]
fn p2_413_shape_proportions() {
    // TODO(srs): P2-413, exact proportions. Reading taken (B styles-themes
    // 6): per the notation page.
    todo!()
}

/// A view drawing every shape, connected both ways, meets CORE-6..9
/// measured on the box rectangle.
#[test]
fn p2_414_core_6_to_9_hold_every_shape() {
    todo!()
}

/// Lines end on the box rectangle, not the outline, for Circle, Ellipse,
/// Hexagon, Diamond and Robot.
#[test]
fn p2_414_per_shape_port_rules() {
    // TODO(srs): P2-414, port rules per shape. Reading taken (B styles-
    // themes 6): lines end on the box rectangle, CORE-6 grid ports hold.
    todo!()
}

/// A `RoundedBox`'s corner radius is larger than a `Box`'s.
#[test]
fn p2_415_rounded_box_radius_larger() {
    todo!()
}

/// `Box` keeps the phase-1 6px radius and `RoundedBox` uses 20px.
#[test]
fn p2_415_box_corner_radius() {
    // TODO(srs): P2-415. Reading taken (B styles-themes 3): Box 6px (phase-1
    // goldens), RoundedBox 20px.
    todo!()
}

/// A box key entry's swatch draws that entry's shape outline, for every
/// shape.
#[test]
fn p2_416_swatch_uses_shape_outline() {
    todo!()
}

// ---- Relationship styles (styles-themes-03)

/// Each of `thickness`, `color`, `colour`, `style`, `dashed`, `routing`,
/// `jump`, `fontSize`, `width`, `position`, `opacity`, `properties` lands
/// in `RelationshipStyle` with its value span.
#[test]
fn p2_12_relationship_style_parsed() {
    todo!()
}

/// `Model::relationship_style_of` resolves per property over
/// `Relationship`, then the declared tags (DSL-15).
#[test]
fn p2_12_cascade_over_relationship_tags() {
    todo!()
}

/// Any other word in a `relationship` block is a DSL-4 error naming it.
#[test]
fn p2_12_unknown_property_rejected() {
    todo!()
}

/// A CSS colour name in a relationship `color`/`colour` resolves to its hex
/// value.
#[test]
fn p2_401_relationship_named_colour() {
    todo!()
}

/// `properties { }` in a relationship style is kept byte for byte and
/// changes no scene.
#[test]
fn p2_410_relationship_style_properties_kept() {
    todo!()
}

/// `dashed true` means `style dashed`, `dashed false` means `style solid`.
#[test]
fn p2_420_dashed_alias() {
    todo!()
}

/// `routing Direct|Orthogonal|Curved` lands in
/// `ResolvedRelationshipStyle::routing`.
#[test]
fn p2_421_routing_parsed() {
    todo!()
}

/// The same file with and without `routing` gives equal scenes.
#[test]
fn p2_421_routing_changes_no_scene() {
    // TODO(srs): P2-421. Reading taken (B styles-themes 1): every value
    // draws with c4rizer's router, no notice.
    todo!()
}

/// `position P` is kept for 0..100; a value outside is clamped.
#[test]
fn p2_425_position_range() {
    // TODO(srs): P2-425 says invalid-value error; B styles-themes 13 [I]
    // clamps. Reading taken: clamp.
    todo!()
}

/// A relationship `opacity N` is kept for 0..100; a value outside is
/// clamped.
#[test]
fn p2_426_opacity_range() {
    // TODO(srs): P2-426 says invalid-value error; B styles-themes 13 [I]
    // clamps. Reading taken: clamp.
    todo!()
}

// ---- Light and dark (styles-themes-04)

/// `light { }` and `dark { }` in `styles` hold `element` and `relationship`
/// blocks with `mode` set.
#[test]
fn p2_21_light_dark_blocks_parsed() {
    todo!()
}

/// Anything but `element`/`relationship` inside `light`/`dark` is a DSL-4
/// error.
#[test]
fn p2_21_other_children_rejected() {
    todo!()
}

/// `style_of_in`/`relationship_style_of_in` use top-level plus `light`
/// blocks in light mode and top-level plus `dark` blocks in dark mode.
#[test]
fn p2_430_mode_selects_blocks() {
    todo!()
}

/// A file with no `light`/`dark` block resolves the same style for every
/// element and relationship in both modes.
#[test]
fn p2_430_no_mode_blocks_same_styles_in_both() {
    todo!()
}

/// For one tag, a mode block beats a top-level block wherever each is
/// written; theme styles apply in both modes.
#[test]
fn p2_430_mode_block_precedence() {
    // TODO(srs): P2-430. Readings taken (B styles-themes 10a, 10b).
    todo!()
}

// ---- Themes (styles-themes-05)

/// `theme <x>` and `themes <x> <y>` inside `views` fill `Model::themes` in
/// order with their `ThemeSource`.
#[test]
fn p2_13_theme_and_themes_parsed() {
    todo!()
}

/// For one tag the file's own block beats the theme's.
#[test]
fn p2_13_file_style_beats_theme_same_tag() {
    todo!()
}

/// For one tag a later theme beats an earlier one.
#[test]
fn p2_13_later_theme_wins() {
    todo!()
}

/// Across tags the DSL-15 tag order decides, whether a style comes from a
/// theme or the file.
#[test]
fn p2_13_tag_order_across_theme_and_file() {
    todo!()
}

/// A local theme JSON file (`elements`/`relationships` arrays with `tag`)
/// read through the `Loader` gives styles with `theme: Some(i)`.
#[test]
fn p2_427_theme_json_read() {
    todo!()
}

/// Unknown JSON keys are ignored, and the theme file is never written.
#[test]
fn p2_427_unknown_keys_ignored() {
    todo!()
}

/// A missing theme is `FileRead`, malformed JSON is `FileFormat`, each
/// naming the file and saying what to fix.
#[test]
fn p2_427_bad_theme_is_parse_error_naming_file() {
    todo!()
}

/// A relative `icon` in a theme resolves against the theme file's folder.
#[test]
fn p2_428_theme_icon_relative_to_theme() {
    todo!()
}

/// A theme by name or URL draws nothing and adds a P2-3 notice naming it.
#[test]
fn p2_429_url_or_name_theme_notice() {
    // TODO(srs): P2-429. Reading taken (B styles-themes 11, approver C):
    // notice, no network; `theme default` draws only if its styles can be
    // bundled under their licence.
    todo!()
}

// ---- Plan and pipeline (styles-themes-06)

/// A resolved `width`/`height` replaces that dimension of the CORE-2 kind
/// default in the plan box.
#[test]
fn p2_402_tag_size_replaces_kind_default() {
    todo!()
}

/// A view's `size` line beats a style `width`/`height`.
#[test]
fn p2_402_size_line_wins() {
    todo!()
}

/// `PlanEdge::style` is the winner's resolved relationship style.
#[test]
fn p2_12_edge_takes_winner_style() {
    todo!()
}

/// `view::plan_in(.., Mode::Dark)` resolves dark blocks and sets
/// `ViewPlan::mode`; `plan` equals `plan_in(.., Mode::Light)`.
#[test]
fn p2_430_plan_in_dark_mode() {
    todo!()
}

/// `pipeline::build_view_in(.., Mode::Dark, ..)` gives a scene with `mode
/// == Mode::Dark`; `build_view` equals light.
#[test]
fn p2_430_build_view_in_dark_mode() {
    todo!()
}

// ---- Box properties in the scene (styles-themes-07)

/// `stroke` sets the box outline colour; with none it is the fill darkened
/// to 72%.
#[test]
fn p2_403_stroke_colour_and_default() {
    todo!()
}

/// `strokeWidth N` gives `SceneBox::stroke.width_px == N`; default 1.
#[test]
fn p2_404_stroke_width_drawn() {
    todo!()
}

/// `border dashed` and `dotted` give dash arrays, dotted shorter than
/// dashed; solid has none.
#[test]
fn p2_405_border_dash_patterns() {
    todo!()
}

/// `opacity N` gives `SceneBox::opacity == N`; default 100.
#[test]
fn p2_406_box_opacity() {
    todo!()
}

/// `fontSize N` draws the box name at N px, and wrapping, L25 fit and
/// EDIT-18 minimum size use it.
#[test]
fn p2_407_font_size_name() {
    todo!()
}

/// Type line and description scale to round(N x 11/14) px on that size plus
/// 4; the name pitch is N + 6.
#[test]
fn p2_407_small_text_scaling() {
    // TODO(srs): P2-407. Reading taken (B styles-themes 5).
    todo!()
}

/// `metadata false` drops the type line, `description false` the
/// description; the rest still fits.
#[test]
fn p2_408_hidden_metadata_and_description() {
    todo!()
}

// ---- Line properties in the scene (styles-themes-11)

/// `thickness N` draws the line and its chevrons N px wide; default 2.
#[test]
fn p2_418_thickness() {
    todo!()
}

/// The arrowhead and join dot scale by thickness/2, never below phase-1
/// size; lane pitch unchanged.
#[test]
fn p2_418_tip_scaling() {
    // TODO(srs): P2-418. Reading taken (B styles-themes 9).
    todo!()
}

/// A relationship colour gives line, chevrons, tip and label text that
/// colour instead of the destination fill.
#[test]
fn p2_419_colour_overrides_core_3() {
    todo!()
}

/// `style dashed|dotted` gives the line's polyline a dash array.
#[test]
fn p2_420_line_dash_styles() {
    todo!()
}

/// With no `style`, lines stay solid (CORE-10).
#[test]
fn p2_420_default_solid() {
    todo!()
}

/// A `jump true` line has a `SceneLine::jumps` point at each crossing with
/// a line drawn before it.
#[test]
fn p2_422_jumps_at_crossings() {
    // TODO(srs): P2-422. Reading taken (B styles-themes 8): hop over lines
    // drawn before it; 6px-radius half circle.
    todo!()
}

/// `jump` leaves `Scene::crossings` and L4 unchanged.
#[test]
fn p2_422_crossing_count_unchanged() {
    todo!()
}

/// When both crossing lines jump, only the later-drawn one hops.
#[test]
fn p2_422_both_jump() {
    // TODO(srs): P2-422. Reading taken (B styles-themes 8).
    todo!()
}

/// A relationship `opacity N` gives `SceneLine::opacity` and
/// `SceneLabel::opacity` N.
#[test]
fn p2_426_line_opacity() {
    todo!()
}

// ---- Labels (styles-themes-10)

/// A relationship `fontSize N` gives `Label::font_px == N`, and label size,
/// clearances and L25 use it.
#[test]
fn p2_423_label_font_size() {
    todo!()
}

/// `Label::line_px` is N + 2.
#[test]
fn p2_423_label_line_pitch() {
    // TODO(srs): P2-423. Reading taken (SRS proposal): N + 2.
    todo!()
}

/// A relationship `width W` wraps the label to W px of text width (CORE-30
/// table).
#[test]
fn p2_424_label_wrapped_to_px_width() {
    todo!()
}

/// `position P` orders candidate centres by distance from P% of the line's
/// length; CORE-14 gaps kept.
#[test]
fn p2_425_position_orders_candidates() {
    todo!()
}

/// With no `position`, placement equals phase 1.
#[test]
fn p2_425_unset_position_is_phase_1() {
    todo!()
}

// ---- Boundaries and groups (styles-themes-12)

/// `Boundary` then `Boundary:SoftwareSystem`/`Boundary:Container` styles
/// set fill, stroke, strokeWidth, color, fontSize, border and opacity of
/// the scope boundary.
#[test]
fn p2_411_boundary_tags_styled() {
    todo!()
}

/// With no boundary style the boundary is the phase-1 one (dashed 8/6, 1px,
/// #666666, no fill).
#[test]
fn p2_411_boundary_default_unchanged() {
    todo!()
}

/// `metadata false` drops the bracketed type from the name tag.
#[test]
fn p2_411_boundary_metadata_false() {
    todo!()
}

/// `shape`, `width`, `height` and `description` on `Boundary*` styles
/// change nothing.
#[test]
fn p2_411_ignored_properties() {
    todo!()
}

/// `Group` then `Group:<name>` styles apply to group boundaries, `metadata`
/// excepted.
#[test]
fn p2_412_group_tags_styled() {
    todo!()
}

/// `element "Boundary:Enterprise"` is accepted, changes no scene, and adds
/// a P2-3 notice naming it.
#[test]
fn p2_440_enterprise_boundary_no_effect() {
    todo!()
}

// ---- Key (styles-themes-13)

/// The key has one line entry per relationship-style tag (last styled
/// non-`Relationship` tag of each drawn winner), named by the tag.
#[test]
fn p2_27_one_entry_per_relationship_style_tag() {
    todo!()
}

/// All other drawn lines share one entry named `Relationship`; a file with
/// no relationship styles has exactly one line entry.
#[test]
fn p2_27_unnamed_lines_single_relationship_entry() {
    todo!()
}

/// Join-dot lines keep their own entry.
#[test]
fn p2_27_join_dot_entry() {
    // TODO(srs): P2-27. Reading taken (B styles-themes 12).
    todo!()
}

/// A named entry's swatch uses that style's resolved colour, thickness and
/// dash.
#[test]
fn p2_434_named_swatch_stroke() {
    todo!()
}

/// The `Relationship` entry's swatch is #707070 when its lines have
/// different colours.
#[test]
fn p2_434_unnamed_swatch_colour() {
    // TODO(srs): P2-434. Reading taken (B styles-themes 12): #707070.
    todo!()
}

/// Boxes differing only in stroke colour, stroke width or border get
/// separate key entries; opacity and icon do not split.
#[test]
fn p2_435_box_look_includes_stroke_and_border() {
    // TODO(srs): P2-435. Reading taken (B styles-themes 12): opacity and
    // icons do not split entries.
    todo!()
}

// ---- Icons (styles-themes-14)

/// `icon <file>` is read through the `Loader` relative to the diagram's
/// folder into `Model::assets` and `ResolvedStyle::icon`; a missing file is
/// `FileRead`.
#[test]
fn p2_409_icon_path_parsed_and_loaded() {
    // TODO(srs): P2-409. Readings taken (B styles-themes 7c, 7e): PNG, JPEG,
    // SVG files and `data:` URIs; missing file is a `FileRead` parse error.
    todo!()
}

/// `SceneBox::icon` lies centred in the bottom band, at most 1/4 of the box
/// height, clear of the texts.
#[test]
fn p2_409_icon_drawn_inside_box_clear_of_text() {
    // TODO(srs): P2-409. Reading taken (B styles-themes 7a).
    todo!()
}

/// The SVG draws the icon as `<image href="data:...">` and the PNG
/// rasterizes it; EXP-1 scan allows only `data:` URIs.
#[test]
fn p2_409_icon_in_svg_and_png() {
    // TODO(srs): P2-409. Reading taken (B styles-themes 7d): a URL icon is
    // not drawn and gets a P2-3 notice.
    todo!()
}

// ---- SVG and PNG (styles-themes-15)

/// The SVG of a fixture using every element property draws each one.
#[test]
fn p2_10_svg_draws_every_property() {
    todo!()
}

/// The SVG draws each shape's outline as the scene gives it.
#[test]
fn p2_11_svg_outline_per_shape() {
    todo!()
}

/// A box at opacity 100 has no opacity attribute; below 100 it has one.
#[test]
fn p2_406_opacity_attribute_only_below_100() {
    todo!()
}

/// A line at opacity 100 has no opacity attribute; below 100 it has one.
#[test]
fn p2_426_line_opacity_attribute_only_below_100() {
    todo!()
}

/// The SVG draws a hop at each `SceneLine::jumps` point.
#[test]
fn p2_422_svg_jump_hops() {
    todo!()
}

/// The PNG of the all-styles fixture is its SVG rasterized (EXP-2).
#[test]
fn p2_436_png_is_svg_rasterized() {
    todo!()
}

// ---- Edits (styles-themes-16)

/// A DSL-13 shape edit writes the reference spelling (`shape
/// MobileDevicePortrait`).
#[test]
fn p2_417_shape_written_in_reference_spelling() {
    todo!()
}

/// A DSL-13 style edit changes only the edited property's line.
#[test]
fn p2_439_edit_keeps_other_properties() {
    todo!()
}

/// On a block using `colour`, a text-colour edit rewrites that line and
/// adds no `color` line.
#[test]
fn p2_439_colour_alias_rewritten_in_place() {
    todo!()
}

/// An `all <tag>` edit on a tag styled only by a theme adds a block to the
/// file's own `styles`.
#[test]
fn p2_439_theme_styles_never_edited() {
    todo!()
}

// ---- End to end (styles-themes-19)

/// `tests/data/styles/all_styles.dsl` (every element property, every shape,
/// every relationship property) builds and exports in both modes.
#[test]
fn p2_436_every_style_fixture_renders() {
    todo!()
}

/// A file with no phase-2 style construct gives the phase-1 scene except
/// the key's line entries; the goldens are regenerated once in styles-
/// themes-13 and re-approved.
#[test]
fn p2_437_no_phase_2_construct_same_scene() {
    todo!()
}

/// A file using every construct of this row round-trips byte-identical
/// (DSL-11).
#[test]
fn p2_438_round_trip_all_styles() {
    todo!()
}

/// Dark mode draws background #111111, box text #ffffff, default fill
/// #333333, line and boundary #aaaaaa.
#[test]
fn p2_431_dark_defaults() {
    // TODO(srs): P2-431. Reading taken (B styles-themes 10c).
    todo!()
}
