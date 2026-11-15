//! Step-4 contract tests of the dsl-breadth row (SRS 4.6: P2-1..7,
//! P2-16..18, P2-120..181). One `#[test]` per requirement, named after
//! it; bodies are `todo!()` until step 5. Fixtures go under
//! `tests/data/dsl-breadth/`. The CLI tests are in
//! `crates/cli/tests/dsl_breadth.rs`, the editor tests in
//! `crates/editor/src/dsl_breadth_tests.rs`.
//!
//! Each `TODO(srs)` names the SRS hole and the reading taken (the
//! Structurizr-faithful one, from docs/plans/phase2-decisions.md Part B,
//! pending the owner's yes).

#![allow(clippy::todo)]

/// `model::GroupId` is a `u32` newtype like `ElementId` (data layout,
/// owner sign-off pending). Serves P2-4.
#[test]
fn group_id_is_4_bytes() {
    assert_eq!(std::mem::size_of::<c4rizer_core::model::GroupId>(), 4);
}

// ---- Grammar and lexing (dsl-breadth-01, -02, -03, -20)

/// tests/data/dsl-breadth/grammar.dsl (plus include and base files) uses every P2-1 keyword in each block the reference allows; `parse_with(FsLoader)` succeeds.
#[test]
fn p2_1_every_reference_keyword_parses() {
    todo!()
}

/// DSL-11 on every file of the grammar fixture: each tree prints back byte for byte.
#[test]
fn p2_1_grammar_fixture_round_trips() {
    todo!()
}

/// Style properties and shapes, light/dark, theme/themes, view expressions and relationship patterns, default, animation, Group/Group:Name style tags: no error, kept byte for byte.
#[test]
fn p2_178_other_row_keywords_accepted_and_kept() {
    todo!()
}

/// A trailing `\` continues the statement; the model equals the one-line form.
#[test]
fn p2_147_line_continuation_joins_lines() {
    todo!()
}

/// A `"""` text block is accepted wherever a string is.
#[test]
fn p2_148_text_block_is_a_string_value() {
    // TODO(srs): P2-148 value rules (/dsl/text-blocks is 404). Reading taken (B dsl-breadth 1): common leading whitespace of non-blank lines removed, no final line break, no escapes.
    todo!()
}

/// `person User` gives the same value as `person "User"`.
#[test]
fn p2_149_unquoted_string_arguments() {
    todo!()
}

/// `!const NAME value` substitutes every later `${NAME}`, inside quoted strings too; the literal's span stays the written bytes.
#[test]
fn p2_150_const_substitutes_in_tokens() {
    todo!()
}

/// A later `!var NAME other` redefines; each `${NAME}` takes the value in force at its line.
#[test]
fn p2_151_var_redefinition_applies_to_later_lines() {
    todo!()
}

/// Redefining a `!const` name is `ModelErrorKind::DuplicateConstant` naming both lines.
#[test]
fn p2_151_const_redefinition_is_error() {
    // TODO(srs): P2-151 whether redefining a `!const` is an error. Reading taken (B dsl-breadth 2): yes, DuplicateConstant.
    todo!()
}

/// A DSL-10 edit of a literal whose written bytes hold `${` is `EditErrorKind::Substituted` and no file changes.
#[test]
fn p2_151_edit_of_substituted_literal_is_refused() {
    // TODO(srs): P2-151 what a DSL-10 edit does to a `${...}` literal. Reading taken (B dsl-breadth 3): refused.
    todo!()
}

/// `${NAME}` with no constant takes `Loader::env(NAME)`; with neither it is left as written.
#[test]
fn p2_152_env_var_and_unknown_name_left_as_written() {
    // TODO(srs): P2-152 precedence. Reading taken (B dsl-breadth 4): an earlier constant or variable wins over the environment.
    todo!()
}

// ---- Body statements and settings (dsl-breadth-03)

/// `tag "X"` in an element or relationship block appends one tag after the earlier ones (DSL-15).
#[test]
fn p2_18_tag_line_appends_one_tag() {
    todo!()
}

/// `url <url>` sets `Element::url` and `Relationship::url`.
#[test]
fn p2_171_url_on_element_and_relationship() {
    todo!()
}

/// workspace, model, configuration, element, relationship, view and views properties land in `workspace_properties`, `model_properties`, `configuration_properties`, `Element::properties`, `Relationship::properties`, `ViewSpec::properties`, `views_properties`, in file order.
#[test]
fn p2_172_properties_stored_in_order_per_owner() {
    todo!()
}

/// One-line and `perspective <name> { ... }` forms both fill Perspective name, description, value, url.
#[test]
fn p2_173_perspectives_inline_and_block() {
    todo!()
}

/// `name` and `description` lines in the workspace block set `Model::name` and `Model::description`.
#[test]
fn p2_174_workspace_name_and_description_lines() {
    todo!()
}

/// `configuration { scope landscape|softwaresystem|none }` sets `Model::configuration_scope`.
#[test]
fn p2_175_configuration_scope_stored() {
    todo!()
}

/// A model that breaks its configuration scope is `ModelErrorKind::ScopeViolation`.
#[test]
fn p2_175_scope_violation_is_error() {
    // TODO(srs): P2-175 the validation the scope applies. Reading taken (B dsl-breadth 20): landscape fails when any system has containers; softwaresystem unless exactly one has; none checks nothing.
    todo!()
}

/// structurizr.locale/timezone/sort/tooltips/title/description/metadata/groups of `views` are in `views_properties`.
#[test]
fn p2_176_view_set_properties_stored() {
    // TODO(srs): P2-176 one requirement per property for its effect (B dsl-breadth 21 lists the readings; effects belong to other rows).
    todo!()
}

// ---- Notices (dsl-breadth-04)

/// One case per P2-3 construct: exactly one `Model::notices` entry, `NoticeKind::NoEffect`, naming the construct, its file and line; no other effect.
#[test]
fn p2_3_no_effect_constructs_yield_notices() {
    todo!()
}

// ---- Deployment, dynamic and new view kinds (dsl-breadth-05)

/// Every P2-120 deployment construct parses; no element they declare is drawn or selected by `include *`.
#[test]
fn p2_120_deployment_constructs_parse_and_are_not_drawn() {
    todo!()
}

/// An undeclared id is `UndeclaredId` (DSL-5); an id already used is `DuplicateId`.
#[test]
fn p2_120_undeclared_or_duplicate_id_in_deployment_is_error() {
    todo!()
}

/// `dynamic <*|system|container>` with ordered steps, relationship-id steps and parallel `{ }` blocks parses.
#[test]
fn p2_121_dynamic_view_body_parses() {
    todo!()
}

/// An undeclared id in a dynamic view's scope or steps is `UndeclaredId`.
#[test]
fn p2_121_undeclared_id_in_dynamic_is_error() {
    todo!()
}

/// `deployment <*|system> <environment>` with every P2-179 body line parses; scope is `ViewScope::Deployment`.
#[test]
fn p2_179_deployment_view_parses() {
    todo!()
}

/// An undeclared scope or environment is `UndeclaredId`.
#[test]
fn p2_179_undeclared_environment_is_error() {
    todo!()
}

/// Keyless systemLandscape, dynamic and deployment views get SystemLandscape-NNN, Dynamic-NNN, Deployment-NNN, per kind in order.
#[test]
fn p2_124_generated_keys_for_new_view_kinds() {
    // TODO(srs): P2-124 keys for custom, image and filtered. Reading taken (B dsl-breadth 22): Custom-NNN, Image-NNN, Filtered-NNN.
    todo!()
}

/// A dynamic or deployment view (and the landscape-filtered kinds until that row lands) is in `Model::views` with its key and title; the rest of the file builds.
#[test]
fn p2_2_unsupported_view_kinds_in_model_views() {
    todo!()
}

// ---- Identifiers (dsl-breadth-06)

/// Under `!identifiers hierarchical`, two `api` containers in s1 and s2 are found as s1.api and s2.api; the same full id twice is `DuplicateId`.
#[test]
fn p2_6_hierarchical_ids_scope_by_parent() {
    todo!()
}

/// Full ids work in relationships, include, exclude, at, size, focus, hide and `!element`.
#[test]
fn p2_142_full_id_in_relationships_views_at_size_focus_hide() {
    // TODO(srs): P2-142 bare id inside the parent's block. Reading taken (B dsl-breadth 11): looked up as <parent full id>.<id> first, then as a full id.
    todo!()
}

/// `r = a -> b` inside s1 is found as `r`, not `s1.r`.
#[test]
fn p2_143_relationship_ids_stay_flat() {
    todo!()
}

/// In flat mode a second element with a used id is `DuplicateId` naming both lines.
#[test]
fn p2_144_flat_duplicate_id_is_error() {
    todo!()
}

/// An element without `<id> =` parses, has key None and `full_key` None, and `include *` selects it as its kind allows.
#[test]
fn p2_145_anonymous_element_selected_by_star() {
    // TODO(srs): P2-145 how at, size and edits address it. Reading taken (B dsl-breadth 10): never; auto-placed, the editor refuses moves/resizes/per-box styles.
    todo!()
}

/// `r = a -> b` sets `Relationship::key` to r and `!relationship r` resolves it.
#[test]
fn p2_146_named_relationship_key() {
    todo!()
}

/// A relationship key declared twice is `DuplicateId` naming both lines.
#[test]
fn p2_146_duplicate_relationship_key_is_error() {
    // TODO(srs): P2-146 relationship id equal to an element id. Reading taken (B dsl-breadth 12): one register, DuplicateId.
    todo!()
}

/// Same-name people/systems, containers in one system, components in one container: `DuplicateName` naming both lines.
#[test]
fn p2_177_duplicate_names_are_errors() {
    // TODO(srs): P2-177 is an open question (Java model rule). Reading taken (B dsl-breadth 13): DuplicateName.
    todo!()
}

// ---- Groups (dsl-breadth-07, -08)

/// `group` in model, a software system and a container: every element inside has `Element::group`.
#[test]
fn p2_4_group_members_carry_group() {
    todo!()
}

/// An element of another level inside a group is `InvalidNesting`.
#[test]
fn p2_127_group_holds_one_level_only() {
    // TODO(srs): P2-127 custom element in a model-level group. Reading taken (B dsl-breadth 14a): allowed.
    todo!()
}

/// `group "<name>"` inside a component block sets that component's group.
#[test]
fn p2_128_group_line_in_component_block() {
    todo!()
}

/// With structurizr.groupSeparator `/`, nested groups A and B give full name A/B.
#[test]
fn p2_129_nested_groups_with_separator() {
    todo!()
}

/// A nested group with no separator set is `NestedGroupNeedsSeparator`.
#[test]
fn p2_129_nested_group_without_separator_is_error() {
    // TODO(srs): P2-129 nested group with no separator. Reading taken (B dsl-breadth 14b): this error.
    todo!()
}

/// A container api inside a group in s1 is s1.api under hierarchical ids.
#[test]
fn p2_131_group_not_in_hierarchical_id() {
    todo!()
}

/// A group with a drawn member gives one `BoundaryOf::Group` boundary around its drawn members (CORE-1 bounds + 40px), its name as tag, no bracketed type.
#[test]
fn p2_130_group_boundary_around_drawn_members() {
    // TODO(srs): P2-130 nested name tag, default colour, structurizr.groups. Readings taken: last segment (Structurizr UI); phase-1 boundary look (B 14c); `structurizr.groups false` hides it (B 14d).
    todo!()
}

/// A nested group's boundary sits inside its enclosing group's boundary; outer before inner in `ViewPlan::boundaries`.
#[test]
fn p2_130_nested_group_boundary_inside_outer() {
    todo!()
}

/// A group of out-of-scope elements does not overlap the scope boundary; one of scope children sits inside it.
#[test]
fn p2_130_out_of_scope_group_clear_of_scope_boundary() {
    todo!()
}

/// A group with no drawn member emits no boundary.
#[test]
fn p2_130_group_without_drawn_member_has_no_boundary() {
    todo!()
}

/// tests/data/dsl-breadth/groups.dsl with no `at` lines: no non-member box overlaps a group rectangle.
#[test]
fn p2_132_auto_place_keeps_non_members_out_of_groups() {
    // TODO(srs): P2-132 whether L26 covers group rectangles. Reading taken (B dsl-breadth 14e): yes.
    todo!()
}

// ---- Custom elements and archetypes (dsl-breadth-09)

/// `element <name> [metadata] [description] [tags]` gives `ElementKind::Custom` with only tag Element and metadata kept; its children are accepted.
#[test]
fn p2_170_custom_element_parsed() {
    // TODO(srs): P2-170 include * selecting custom elements. Reading taken (B dsl-breadth 16): never on context/container/component views; explicit include adds one.
    todo!()
}

/// `service = container { technology "Go"; tags "Service" }`: `x = service "Name"` is a container with technology Go and tag Service; positional/body values override.
#[test]
fn p2_155_element_archetype_defaults() {
    todo!()
}

/// `api = service { ... }` takes the base defaults and overrides its own.
#[test]
fn p2_156_archetype_inheritance() {
    // TODO(srs): P2-156 tag combination. Reading taken (B dsl-breadth 15a): base tags then its own.
    todo!()
}

/// `sync = -> { tags "Synchronous" }`, `https = --sync-> { technology "HTTPS" }`: `a --https-> b "d"` has tag Synchronous, technology HTTPS, description d.
#[test]
fn p2_157_relationship_archetype_arrow() {
    todo!()
}

/// A base outside the P2-158 list is `InvalidArchetype` naming it.
#[test]
fn p2_158_archetype_base_types() {
    // TODO(srs): P2-158 tag order of an element from an archetype. Reading taken (B dsl-breadth 15b): Element, kind tag, archetype chain root down, own tags.
    todo!()
}

/// `--name->` naming no relationship archetype is `UnknownArchetype` naming it.
#[test]
fn p2_158_unknown_relationship_archetype_is_error() {
    todo!()
}

/// Technology defaults apply only to containers and components, metadata only to custom elements.
#[test]
fn p2_181_technology_and_metadata_defaults_by_kind() {
    // TODO(srs): P2-181 technology on another base. Reading taken (B dsl-breadth 15c): UnknownConstruct naming `technology`.
    todo!()
}

// ---- Directives that modify the model (dsl-breadth-10, -11)

/// `!element <id> { ... }` adds tags, properties, children and relationships to that element.
#[test]
fn p2_17_element_directive_adds_tags_children_relationships() {
    todo!()
}

/// `!relationship <id> { ... }` adds tag, tags, url, properties, perspectives.
#[test]
fn p2_167_relationship_directive_adds_tag_url_properties() {
    todo!()
}

/// `!extend` and `!ref` behave as `!element`, or `!relationship` for a relationship id.
#[test]
fn p2_169_extend_and_ref_are_aliases() {
    // TODO(srs): P2-169 `<id> = !element <canonical name>`. Reading taken (B dsl-breadth 23): binds to the item with that canonical name; unknown is UndeclaredId.
    todo!()
}

/// `!elements <expr> { ... }` applies its children to every element `Model::select_elements` returns; one case per element-expression form, plus && and ||.
#[test]
fn p2_166_elements_directive_per_expression_form() {
    // TODO(srs): mixed && and ||. Reading taken (B dsl-breadth 25): MixedOperators.
    todo!()
}

/// `!relationships <expr> { ... }` applies its children to every relationship `Model::select_relationships` returns; one case per relationship-expression form, plus && and ||.
#[test]
fn p2_168_relationships_directive_per_expression_form() {
    todo!()
}

// ---- Implied relationships (dsl-breadth-12)

/// With `!impliedRelationships false`, no lifted line is drawn.
#[test]
fn p2_7_implied_false_draws_no_lifted_line() {
    todo!()
}

/// With `!impliedRelationships false`, `include *` counts only relationships declared on the element itself.
#[test]
fn p2_7_implied_false_star_counts_direct_only() {
    // TODO(srs): P2-7 a directive between relationships. Reading taken (B dsl-breadth 17): applies to later relationships only.
    todo!()
}

/// `!impliedRelationships true` gives the same builds as no directive (CORE-5).
#[test]
fn p2_153_implied_true_equals_default() {
    todo!()
}

/// `!impliedRelationships <fqcn>` behaves as default and adds one notice.
#[test]
fn p2_154_implied_fqcn_default_with_notice() {
    todo!()
}

// ---- Terminology (dsl-breadth-13)

/// `terminology { softwareSystem "Service" }` (and person, container, component) replaces the word on box type lines and boundary tags.
#[test]
fn p2_161_terminology_replaces_type_words() {
    // TODO(srs): P2-161 External Software System. Reading taken (B dsl-breadth 18b): `External <word>`.
    todo!()
}

/// `terminology { relationship "Link" }` makes the key's line entries read Link.
#[test]
fn p2_162_terminology_relationship_key_label() {
    todo!()
}

/// metadata round/curly/angle/none gives (…), {…}, <…>, no brackets; square is default.
#[test]
fn p2_163_terminology_metadata_brackets() {
    // TODO(srs): P2-163 double-angle glyphs. Reading taken (B dsl-breadth 18a): « and ».
    todo!()
}

// ---- Docs and ADRs (dsl-breadth-14)

/// `!docs <path> [fqcn]` on workspace, software system, container, component records an Attachment on that owner.
#[test]
fn p2_159_docs_path_recorded_on_owner() {
    // TODO(srs): P2-159 whether phase 2 reads or checks the files. Reading taken (B dsl-breadth 19): a missing path or one outside the folder is an error; content not read.
    todo!()
}

/// `!adrs <path> [type|fqcn]` and `!decisions` record path and importer, `AttachmentKind::Adrs`.
#[test]
fn p2_160_adrs_and_decisions_recorded() {
    todo!()
}

// ---- !include and workspace extends (dsl-breadth-15, -16)

/// `!include <relative file>` gives the same model as the text pasted in place.
#[test]
fn p2_5_include_file_inlines_statements() {
    todo!()
}

/// `!include <dir>` inlines every DSL file of the directory.
#[test]
fn p2_133_include_directory() {
    // TODO(srs): P2-133 order, recursion, extensions. Reading taken (B dsl-breadth 5): recursive, ascending relative path byte order, dot-names skipped.
    todo!()
}

/// A path resolving (after `..` and symlinks, `Loader::canonicalize`) outside the including file's directory is `PathOutsideRoot`.
#[test]
fn p2_134_include_outside_parent_dir_is_error() {
    todo!()
}

/// Included files get `FileId` 1.. in first-read order, `Parsed::paths` is indexed by `FileId`, every span names its file, each tree prints back byte for byte.
#[test]
fn p2_135_included_files_numbered_and_round_trip() {
    todo!()
}

/// A parse or model error in an included file carries that file's `FileId`, line and column.
#[test]
fn p2_136_error_in_included_file_names_it() {
    todo!()
}

/// A cycle, self-include included, is `IncludeCycle` naming the closing file, and never hangs.
#[test]
fn p2_137_include_cycle_is_error() {
    todo!()
}

/// `!include http://...` is `InsecureUrl` naming the URL.
#[test]
fn p2_138_non_https_url_include_is_error() {
    // TODO(srs): P2-138 HTTPS fetch/cache/offline. Reading taken (B dsl-breadth 6): UrlNotSupported.
    todo!()
}

/// `workspace extends <relative file>` puts the base's elements, relationships and views in the model.
#[test]
fn p2_16_extends_loads_base() {
    // TODO(srs): P2-16 URL and JSON bases, edits to base literals. Readings taken (B dsl-breadth 6, 9): UrlNotSupported; JSON base LaterPhase (3); edits patch the base file.
    todo!()
}

/// A relationship or view in the extending file may reference a base id.
#[test]
fn p2_164_child_references_base_ids() {
    todo!()
}

/// `!element <base id> { tags "X" }` adds tag X to the base element.
#[test]
fn p2_165_element_directive_modifies_base() {
    todo!()
}

// ---- Edits across files (dsl-breadth-17)

/// A DSL-10 edit of a literal from an included file patches that file only (`Applied::files`); `Applied::text` equals the root input.
#[test]
fn p2_139_text_edit_patches_included_file() {
    // TODO(srs): P2-139 per-box tags line for an included element. Reading taken (B dsl-breadth 8): the file holding the declaration.
    todo!()
}

/// An at or size edit of a view declared in the root file is written to the root file.
#[test]
fn p2_140_at_and_size_go_to_root_file() {
    // TODO(srs): P2-140 a view declared in an included file. Reading taken (B dsl-breadth 7): the file holding the view block.
    todo!()
}

/// Under hierarchical ids every edit element key is `Model::full_key` and edit writes the full id.
#[test]
fn p2_142_edit_keys_are_full_hierarchical_ids() {
    todo!()
}

// ---- Pipeline (dsl-breadth-18)

/// `build_all` lists every non-drawable view in `AllViews::unsupported` (key, title, kind) and builds the rest as before; `has_errors` is unaffected.
#[test]
fn p2_2_build_all_lists_unsupported_views() {
    todo!()
}

/// `build_view` on a non-drawable key is `BuildError::Unsupported`, never a panic; nothing is planned or routed.
#[test]
fn p2_2_build_view_on_unsupported_key_is_error() {
    todo!()
}
