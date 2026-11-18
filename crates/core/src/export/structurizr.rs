//! The Structurizr view of a c4rizer model, shared by every
//! Structurizr-faithful output (SRS 4.7, row exporters: P3-406..P3-415):
//! numeric ids in Structurizr creation order, implied and copied
//! relationships, each view's elements and relationship views in
//! Structurizr's order, element and relationship styles as Structurizr
//! resolves them, names and type lines.
//!
//! Depends on: `model`.

// Shape of the module: `Workspace::new` walks the model once and gives
// every element and relationship the id Structurizr 6.2.4's DSL parser
// would give it (one counter, shared by elements and relationships, in
// creation order); `Workspace::view` gives one view's content as
// Structurizr's exporters read it. Nothing here is pub outside `export`.
//
// DECISION: creation order is the source order of the declarations: an
// element and a written relationship in one file compare by byte offset;
// across files (an `!include`, a `workspace extends` base) a relationship
// is created as soon as both its ends exist, which is the order the
// builder walks them in.
// DECISION: an implied relationship is created right after the
// relationship it comes from, by `CreateImpliedRelationshipsUnless
// AnyRelationshipExistsStrategy` (source and its ancestors, outer loop;
// destination and its ancestors, inner loop), never for a deployment
// element, whose relationships Structurizr does not lift (reference
// `deployment/json`: `dns -> lb` is id 33, `lb -> webBlue` 34).
// DECISION: a copied instance relationship (P3-106) is created right
// after the later of its two instances, as Structurizr replicates when an
// instance is added.

use std::collections::BTreeSet;

use crate::model::{
    Border, Colour, ElementId, ElementKind, ImpliedRelationships, Mode, Model, RelationshipId, Selector,
    Shape, ViewKind, ViewScope, ViewSpec, is_relationship_expression,
};

/// One relationship as Structurizr has it: written, implied or copied.
#[derive(Debug, Clone)]
pub(crate) struct Rel {
    /// Structurizr's numeric id.
    pub(crate) id: u32,
    pub(crate) source: ElementId,
    pub(crate) destination: ElementId,
    /// Empty when none.
    pub(crate) description: String,
    /// Empty when none.
    pub(crate) technology: String,
    /// The c4rizer relationship this one is (written or copied), `None`
    /// for an implied one.
    pub(crate) own: Option<RelationshipId>,
    /// Structurizr's `linkedRelationshipId`: the relationship an implied
    /// or copied one comes from.
    pub(crate) linked: Option<u32>,
}

/// One relationship view of a view, in drawing order.
#[derive(Debug, Clone)]
pub(crate) struct ViewRel {
    /// Index into `Workspace::rels`.
    pub(crate) rel: usize,
    /// The step number of a dynamic view, `None` elsewhere.
    pub(crate) order: Option<String>,
    /// The description drawn: the step's own, else the relationship's.
    pub(crate) description: String,
    /// The relationship's source and destination (Structurizr's
    /// `relationshipView.getRelationship()`; a response swaps them where
    /// the format does).
    pub(crate) source: ElementId,
    pub(crate) destination: ElementId,
    /// Structurizr `RelationshipView.response`.
    pub(crate) response: bool,
}

/// One view's content as Structurizr's exporters read it.
#[derive(Debug, Clone)]
pub(crate) struct View<'m> {
    pub(crate) spec: &'m ViewSpec,
    /// The view's elements, deployment nodes included, sorted as
    /// Structurizr sorts them (by id, as strings).
    pub(crate) elements: Vec<ElementId>,
    pub(crate) relationships: Vec<ViewRel>,
}

/// Structurizr's resolved element style (`ElementStyle`, defaults of
/// Structurizr 6.2.4's exporters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) background: Colour,
    pub(crate) color: Colour,
    pub(crate) stroke: Colour,
    pub(crate) shape: Shape,
    pub(crate) font_size: u32,
    pub(crate) width: i32,
    pub(crate) stroke_width: u8,
    pub(crate) border: Border,
    /// `Element` and the element's tags that a style block names, in tag
    /// order: what the `PlantUML` class names.
    pub(crate) tags: Vec<String>,
    /// `strokeWidth` as written, if any.
    pub(crate) stroke_width_set: Option<u8>,
    /// `metadata` and `description` (both true by default).
    pub(crate) metadata: bool,
    pub(crate) description: bool,
    /// The `plantuml.shadow` style property is `true`.
    pub(crate) shadow: bool,
    /// The icon as Structurizr holds it (a file icon is a `data:` URI).
    pub(crate) icon: Option<String>,
    /// The `ilograph.icon` style property.
    pub(crate) ilograph_icon: Option<String>,
}

/// The style blocks of one tag merged (`TagStyle` of `tag_style`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TagStyle {
    pub(crate) background: Option<Colour>,
    pub(crate) stroke: Option<Colour>,
    pub(crate) color: Option<Colour>,
    pub(crate) stroke_width: Option<u8>,
    pub(crate) border: Option<Border>,
    pub(crate) font_size: Option<u32>,
    pub(crate) icon: Option<String>,
    pub(crate) shadow: bool,
}

/// Structurizr's resolved relationship style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelStyle {
    pub(crate) thickness: u32,
    pub(crate) color: Colour,
    pub(crate) style: Border,
    pub(crate) font_size: u32,
    pub(crate) width: u32,
    /// `Relationship` and the styled tags, in tag order.
    pub(crate) tags: Vec<String>,
}

pub(crate) const WHITE: Colour = Colour { r: 0xff, g: 0xff, b: 0xff };
pub(crate) const GREY: Colour = Colour { r: 0x44, g: 0x44, b: 0x44 };

/// Structurizr's `darker`: each channel times 0.7, rounded down.
// DECISION: computed in `f64` and truncated, as Java does: 0x5a * 0.7 is
// 62.99..., so `#5d5a6b` darkens to `#413e4a` (reference outputs).
pub(crate) fn darker(c: Colour) -> Colour {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=178.5
    let f = |v: u8| (f64::from(v) * 0.7) as u8;
    Colour { r: f(c.r), g: f(c.g), b: f(c.b) }
}

/// The model with Structurizr's ids.
#[derive(Debug)]
pub(crate) struct Workspace<'m> {
    pub(crate) model: &'m Model,
    /// Structurizr id per `ElementId`.
    ids: Vec<u32>,
    /// Every relationship, in id order.
    pub(crate) rels: Vec<Rel>,
    /// The next id to give.
    next: u32,
}

impl<'m> Workspace<'m> {
    /// Allocates the ids (module comment).
    pub(crate) fn new(model: &'m Model) -> Workspace<'m> {
        let mut ws = Workspace {
            model,
            ids: vec![0; model.elements.len()],
            rels: Vec::new(),
            next: 1,
        };
        let written: Vec<RelationshipId> = model
            .relationships
            .iter()
            .filter(|r| r.copied_from.is_none())
            .map(|r| r.id)
            .collect();
        let mut created = vec![false; model.elements.len()];
        let (mut at_element, mut at_written) = (0usize, 0usize);
        while at_element < model.elements.len() || at_written < written.len() {
            let take_element = if at_element >= model.elements.len() {
                false
            } else if at_written >= written.len() {
                true
            } else {
                let el = &model.elements[at_element];
                let r = model.relationship(written[at_written]);
                if el.span.file == r.span.file {
                    el.span.start <= r.span.start
                } else {
                    !(created[r.source.value.0 as usize] && created[r.destination.value.0 as usize])
                }
            };
            if take_element {
                let id = model.elements[at_element].id;
                ws.ids[at_element] = ws.next;
                ws.next += 1;
                created[at_element] = true;
                // Copies whose later instance this is.
                for r in model.relationships.iter().filter(|r| r.copied_from.is_some()) {
                    if r.source.value.max(r.destination.value) == id {
                        let linked = r.copied_from.and_then(|o| ws.rel_of(o)).map(|i| ws.rels[i].id);
                        ws.push((r.source.value, r.destination.value), r.id, Some(r.id), linked);
                    }
                }
                at_element += 1;
            } else {
                let r = model.relationship(written[at_written]);
                let origin_id = ws.push((r.source.value, r.destination.value), r.id, Some(r.id), None);
                if r.implied_relationships != ImpliedRelationships::False
                    && !is_deployment(model, r.source.value)
                    && !is_deployment(model, r.destination.value)
                {
                    let mut s = Some(r.source.value);
                    while let Some(sv) = s {
                        let mut d = Some(r.destination.value);
                        while let Some(dv) = d {
                            if !ws.exists(sv, dv) && allowed(model, sv, dv) {
                                ws.push((sv, dv), r.id, None, Some(origin_id));
                            }
                            d = model.element(dv).parent;
                        }
                        s = model.element(sv).parent;
                    }
                }
                at_written += 1;
            }
        }
        ws
    }

    /// Adds a relationship between `ends` with the next id, which it
    /// returns.
    fn push(
        &mut self,
        (source, destination): (ElementId, ElementId),
        origin: RelationshipId,
        own: Option<RelationshipId>,
        linked: Option<u32>,
    ) -> u32 {
        let id = self.next;
        self.next += 1;
        let r = self.model.relationship(origin);
        let text = |l: &Option<crate::model::Literal>| l.as_ref().map(|l| l.value.clone()).unwrap_or_default();
        self.rels.push(Rel {
            id,
            source,
            destination,
            description: text(&r.description),
            technology: text(&r.technology),
            own,
            linked,
        });
        id
    }

    fn exists(&self, source: ElementId, destination: ElementId) -> bool {
        self.rels.iter().any(|r| r.source == source && r.destination == destination)
    }

    /// The index in `rels` of c4rizer relationship `r` (written or
    /// copied).
    pub(crate) fn rel_of(&self, r: RelationshipId) -> Option<usize> {
        self.rels.iter().position(|x| x.own == Some(r))
    }

    /// Structurizr's id of `e`.
    pub(crate) fn id(&self, e: ElementId) -> u32 {
        self.ids[e.0 as usize]
    }

    /// The written (or copied) tags of relationship `rels[i]`: an implied
    /// relationship has none.
    pub(crate) fn rel_tags(&self, i: usize) -> Vec<String> {
        match self.rels[i].own {
            Some(r) if self.model.relationship(r).copied_from.is_none() => {
                self.model.relationship(r).tags.clone()
            }
            _ => Vec::new(),
        }
    }

    /// Sorts `elements` as Structurizr's exporters do: by id, compared
    /// as strings.
    pub(crate) fn sort(&self, elements: &mut [ElementId]) {
        elements.sort_by_cached_key(|&e| self.id(e).to_string());
    }

    /// The content of `spec` as Structurizr would hold it; `None` for a
    /// kind no Structurizr text exporter writes (filtered, image).
    pub(crate) fn view(&self, spec: &'m ViewSpec) -> Option<View<'m>> {
        let model = self.model;
        match spec.kind {
            ViewKind::Filtered | ViewKind::Image => None,
            ViewKind::Dynamic => {
                let mut elements: Vec<ElementId> = model.included(spec);
                self.sort(&mut elements);
                let mut relationships = Vec::new();
                for i in spec.drawn_steps() {
                    let step = &spec.steps[i];
                    let (s, d) = if step.response {
                        (step.destination.value, step.source.value)
                    } else {
                        (step.source.value, step.destination.value)
                    };
                    let tech = step.technology.as_ref().map(|t| t.value.as_str());
                    let found = self
                        .rels
                        .iter()
                        .position(|r| {
                            r.source == s && r.destination == d && tech.is_none_or(|t| r.technology == t)
                        })
                        .or_else(|| self.rel_of(step.relationship));
                    let Some(rel) = found else { continue };
                    let description = step
                        .description
                        .as_ref()
                        .map_or_else(|| self.rels[rel].description.clone(), |d| d.value.clone());
                    relationships.push(ViewRel {
                        rel,
                        order: Some(step.order.clone()),
                        description,
                        source: self.rels[rel].source,
                        destination: self.rels[rel].destination,
                        response: step.response,
                    });
                }
                Some(View { spec, elements, relationships })
            }
            _ => {
                let mut set: BTreeSet<ElementId> = model.included(spec).into_iter().collect();
                if spec.kind == ViewKind::Deployment {
                    // Structurizr adds the deployment nodes around each
                    // shown element.
                    for e in set.clone() {
                        let mut p = model.element(e).parent;
                        while let Some(x) = p {
                            set.insert(x);
                            p = model.element(x).parent;
                        }
                    }
                }
                let excluded = self.excluded(spec);
                let mut elements: Vec<ElementId> = set.iter().copied().collect();
                self.sort(&mut elements);
                let mut rels: Vec<usize> = (0..self.rels.len())
                    .filter(|&i| {
                        let r = &self.rels[i];
                        set.contains(&r.source)
                            && set.contains(&r.destination)
                            && !r.own.is_some_and(|o| excluded.contains(&o))
                    })
                    .collect();
                rels.sort_by_cached_key(|&i| self.rels[i].id.to_string());
                let relationships = rels
                    .into_iter()
                    .map(|rel| ViewRel {
                        rel,
                        order: None,
                        description: self.rels[rel].description.clone(),
                        source: self.rels[rel].source,
                        destination: self.rels[rel].destination,
                        response: false,
                    })
                    .collect();
                Some(View { spec, elements, relationships })
            }
        }
    }

    /// The written relationships an `exclude` line of `spec` names.
    // TODO(srs): P3-406, `exclude` of a relationship that implied ones
    // come from. Reading taken: only the named relationship leaves the
    // view, as Structurizr removes one relationship view per match.
    fn excluded(&self, spec: &ViewSpec) -> BTreeSet<RelationshipId> {
        let mut out = BTreeSet::new();
        for s in &spec.exclude {
            match &s.value {
                Selector::Relationship(r) => {
                    out.insert(*r);
                }
                Selector::Expression(e) if is_relationship_expression(e) => {
                    out.extend(self.model.select_relationships(e));
                }
                _ => {}
            }
        }
        out
    }

    /// Structurizr's `View.getName()`: the default title.
    pub(crate) fn view_name(&self, spec: &ViewSpec) -> String {
        let model = self.model;
        let name = |e: ElementId| model.element(e).name.value.clone();
        let scoped = |e: ElementId| match model.element(e).parent {
            Some(p) if model.element(e).kind == ElementKind::Container => format!("{} - {}", name(p), name(e)),
            _ => name(e),
        };
        match (&spec.kind, &spec.scope) {
            (ViewKind::SystemLandscape, _) => "System Landscape View".to_owned(),
            (ViewKind::SystemContext, ViewScope::Element(e)) => format!("System Context View: {}", name(e.value)),
            (ViewKind::Container, ViewScope::Element(e)) => format!("Container View: {}", name(e.value)),
            (ViewKind::Component, ViewScope::Element(e)) => {
                let c = e.value;
                let system = model.element(c).parent.map(name).unwrap_or_default();
                format!("Component View: {system} - {}", name(c))
            }
            (ViewKind::Dynamic, ViewScope::Element(e)) => format!("Dynamic View: {}", scoped(e.value)),
            (ViewKind::Dynamic, _) => "Dynamic View".to_owned(),
            (
                ViewKind::Deployment,
                ViewScope::Deployment {
                    software_system,
                    environment,
                },
            ) => {
                let env = self.environment_name(&environment.value);
                match software_system {
                    Some(s) => format!("Deployment View: {} - {env}", name(s.value)),
                    None => format!("Deployment View: {env}"),
                }
            }
            (ViewKind::Custom, _) => "Custom View".to_owned(),
            _ => spec.key.clone(),
        }
    }

    /// The environment's name for `written` (its name or its id).
    fn environment_name(&self, written: &str) -> String {
        self.model
            .environments
            .iter()
            .find(|e| e.name.value == written)
            .or_else(|| {
                self.model
                    .environments
                    .iter()
                    .find(|e| e.key.as_ref().is_some_and(|k| k.value.eq_ignore_ascii_case(written)))
            })
            .map_or_else(|| written.to_owned(), |e| e.name.value.clone())
    }

    /// The diagram title: the view's `title`, else its name.
    pub(crate) fn title(&self, spec: &ViewSpec) -> String {
        spec.title
            .as_ref()
            .map_or_else(|| self.view_name(spec), |t| t.value.clone())
    }

    /// The view's description, `None` when empty.
    pub(crate) fn description(spec: &ViewSpec) -> Option<&str> {
        spec.description
            .as_ref()
            .map(|d| d.value.as_str())
            .filter(|d| !d.is_empty())
    }

    /// The name Structurizr shows for `e`: an instance shows its
    /// element's name.
    pub(crate) fn name(&self, e: ElementId) -> &str {
        let el = self.model.element(e);
        match el.instance_of {
            Some(of) => &self.model.element(of).name.value,
            None => &el.name.value,
        }
    }

    /// The description Structurizr shows for `e` (an instance shows its
    /// element's); empty when none.
    pub(crate) fn element_description(&self, e: ElementId) -> &str {
        let el = self.model.element(e);
        let el = el.instance_of.map_or(el, |of| self.model.element(of));
        el.description.as_ref().map_or("", |d| d.value.as_str())
    }

    /// The technology Structurizr shows for `e`; empty when none.
    pub(crate) fn technology(&self, e: ElementId) -> &str {
        let el = self.model.element(e);
        let el = el.instance_of.map_or(el, |of| self.model.element(of));
        el.technology.as_ref().map_or("", |d| d.value.as_str())
    }

    /// The element's kind name (`Person`, `Container`, ...): an instance
    /// is named as its element, terminology applied.
    pub(crate) fn kind_name(&self, e: ElementId) -> String {
        let el = self.model.element(e);
        let el = el.instance_of.map_or(el, |of| self.model.element(of));
        let t = &self.model.terminology;
        let term = |l: &Option<crate::model::Literal>, d: &str| {
            l.as_ref().map_or_else(|| d.to_owned(), |l| l.value.clone())
        };
        match el.kind {
            ElementKind::Person => term(&t.person, "Person"),
            ElementKind::SoftwareSystem => term(&t.software_system, "Software System"),
            ElementKind::Container => term(&t.container, "Container"),
            ElementKind::Component => term(&t.component, "Component"),
            ElementKind::DeploymentNode => term(&t.deployment_node, "Deployment Node"),
            ElementKind::InfrastructureNode => term(&t.infrastructure_node, "Infrastructure Node"),
            _ => el
                .metadata
                .as_ref()
                .map_or_else(|| "Element".to_owned(), |m| m.value.clone()),
        }
    }

    /// Structurizr's type line without brackets: `Person`, `Container:
    /// Rust`.
    pub(crate) fn type_line(&self, e: ElementId) -> String {
        let kind = self.kind_name(e);
        let tech = self.technology(e);
        let el = self.model.element(e);
        let el = el.instance_of.map_or(el, |of| self.model.element(of));
        let has_tech = matches!(
            el.kind,
            ElementKind::Container | ElementKind::Component | ElementKind::DeploymentNode | ElementKind::InfrastructureNode
        );
        if has_tech && !tech.is_empty() {
            format!("{kind}: {tech}")
        } else {
            kind
        }
    }

    /// The `PlantUML` alias of `e` (`idOf`): names with non-word characters
    /// removed, joined with `.` from the top; a deployment node at the
    /// top starts with its environment; an instance ends `_<n>`.
    pub(crate) fn alias(&self, e: ElementId) -> String {
        let el = self.model.element(e);
        match el.parent {
            None if el.kind == ElementKind::DeploymentNode => {
                let env = el
                    .environment
                    .map(|i| self.model.environments[i.0 as usize].name.value.clone())
                    .unwrap_or_default();
                format!("{}.{}", word(&env), word(&el.name.value))
            }
            None => word(&el.name.value),
            Some(p) => match el.instance_of {
                Some(_) => format!("{}.{}_{}", self.alias(p), word(self.name(e)), el.instance_number.unwrap_or(1)),
                None => format!("{}.{}", self.alias(p), word(&el.name.value)),
            },
        }
    }

    /// The element style of `e` as Structurizr resolves it.
    pub(crate) fn style(&self, e: ElementId) -> Style {
        let model = self.model;
        let mut tags = vec!["Element".to_owned()];
        let mut background = None;
        let mut color = None;
        let mut stroke = None;
        let mut style = Style {
            background: WHITE,
            color: GREY,
            stroke: GREY,
            shape: Shape::Box,
            font_size: 24,
            width: 450,
            stroke_width: 2,
            border: Border::Solid,
            tags: Vec::new(),
            stroke_width_set: None,
            metadata: true,
            description: true,
            shadow: false,
            icon: None,
            ilograph_icon: None,
        };
        for i in model.styles_for(e) {
            let b = &model.element_styles[i];
            if !tags.contains(&b.tag.value) {
                tags.push(b.tag.value.clone());
            }
            if let Some(c) = b.background {
                background = Some(c.value);
            }
            if let Some(c) = b.color {
                color = Some(c.value);
            }
            if let Some(c) = b.stroke {
                stroke = Some(c.value);
            }
            if let Some(s) = b.shape {
                style.shape = s.value;
            }
            if let Some(f) = b.font_size {
                style.font_size = f.value;
            }
            if let Some(w) = b.width {
                style.width = w.value;
            }
            if let Some(w) = b.stroke_width {
                style.stroke_width = w.value;
                style.stroke_width_set = Some(w.value);
            }
            if let Some(x) = b.border {
                style.border = x.value;
            }
            if let Some(m) = b.metadata {
                style.metadata = m.value;
            }
            if let Some(d) = b.description {
                style.description = d.value;
            }
            if let Some(i) = &b.icon {
                style.icon = Some(icon_uri(model, &i.value));
            }
            for p in &b.properties {
                match p.name.value.as_str() {
                    "plantuml.shadow" => style.shadow = p.value.value.eq_ignore_ascii_case("true"),
                    "ilograph.icon" => style.ilograph_icon = Some(p.value.value.clone()),
                    _ => {}
                }
            }
        }
        style.background = background.unwrap_or(WHITE);
        style.color = color.unwrap_or(GREY);
        style.stroke = stroke.unwrap_or_else(|| background.map_or(GREY, darker));
        style.tags = tags;
        style
    }

    /// Structurizr's `findElementStyle(tag)`: the blocks for exactly
    /// `tag` (themes first, light mode) merged; `None` when there is none.
    pub(crate) fn tag_style(&self, tag: &str) -> Option<TagStyle> {
        let model = self.model;
        let light = |m: Option<Mode>| m.is_none_or(|m| m == Mode::Light);
        let blocks: Vec<&crate::model::ElementStyle> =
            model.element_styles.iter().filter(|b| b.tag.value == tag && light(b.mode)).collect();
        if blocks.is_empty() {
            return None;
        }
        let (theme, file): (Vec<_>, Vec<_>) = blocks.into_iter().partition(|b| b.theme.is_some());
        let mut out = TagStyle::default();
        for b in theme.into_iter().chain(file) {
            if let Some(v) = b.background {
                out.background = Some(v.value);
            }
            if let Some(v) = b.stroke {
                out.stroke = Some(v.value);
            }
            if let Some(v) = b.color {
                out.color = Some(v.value);
            }
            if let Some(v) = b.stroke_width {
                out.stroke_width = Some(v.value);
            }
            if let Some(v) = b.border {
                out.border = Some(v.value);
            }
            if let Some(v) = b.font_size {
                out.font_size = Some(v.value);
            }
            if let Some(i) = &b.icon {
                out.icon = Some(icon_uri(model, &i.value));
            }
            for p in &b.properties {
                if p.name.value == "plantuml.shadow" {
                    out.shadow = p.value.value.eq_ignore_ascii_case("true");
                }
            }
        }
        Some(out)
    }

    /// The relationship style of `rels[i]` as Structurizr resolves it:
    /// the tags of the relationships it is linked to (implied from,
    /// copied from) come first.
    pub(crate) fn rel_style(&self, i: usize) -> RelStyle {
        let model = self.model;
        let mut chain = vec![i];
        let mut at = i;
        while let Some(l) = self.rels[at].linked {
            match self.rels.iter().position(|r| r.id == l) {
                Some(j) if !chain.contains(&j) => {
                    chain.push(j);
                    at = j;
                }
                _ => break,
            }
        }
        let mut own = vec!["Relationship".to_owned()];
        for &j in chain.iter().rev() {
            for t in self.rel_tags(j) {
                own.push(t);
            }
        }
        let mut style = RelStyle {
            thickness: 2,
            color: GREY,
            style: Border::Dashed,
            font_size: 24,
            width: 200,
            tags: vec!["Relationship".to_owned()],
        };
        let light = |m: Option<Mode>| m.is_none_or(|m| m == Mode::Light);
        for tag in &own {
            let blocks = model
                .relationship_styles
                .iter()
                .filter(|b| &b.tag.value == tag && light(b.mode));
            // Theme blocks first, then the file's (P2-13).
            let (theme, file): (Vec<_>, Vec<_>) = blocks.partition(|b| b.theme.is_some());
            for b in theme.into_iter().chain(file) {
                if !style.tags.contains(tag) {
                    style.tags.push(tag.clone());
                }
                if let Some(v) = b.thickness {
                    style.thickness = v.value;
                }
                if let Some(v) = b.color {
                    style.color = v.value;
                }
                if let Some(v) = b.style {
                    style.style = v.value;
                }
                if let Some(v) = b.font_size {
                    style.font_size = v.value;
                }
                if let Some(v) = b.width {
                    style.width = v.value;
                }
            }
        }
        style
    }

    /// The c4rizer `instances` text of deployment node `e`, if any.
    pub(crate) fn instances(&self, e: ElementId) -> Option<&str> {
        self.model.element(e).instances.as_ref().map(|i| i.value.as_str())
    }
}

/// True for a deployment node, infrastructure node or instance.
pub(crate) fn is_deployment(model: &Model, e: ElementId) -> bool {
    matches!(
        model.element(e).kind,
        ElementKind::DeploymentNode
            | ElementKind::InfrastructureNode
            | ElementKind::SoftwareSystemInstance
            | ElementKind::ContainerInstance
    )
}

/// `impliedRelationshipIsAllowed`: two different elements, neither inside
/// the other.
fn allowed(model: &Model, s: ElementId, d: ElementId) -> bool {
    let inside = |a: ElementId, b: ElementId| {
        let mut p = model.element(a).parent;
        while let Some(x) = p {
            if x == b {
                return true;
            }
            p = model.element(x).parent;
        }
        false
    };
    s != d && !inside(s, d) && !inside(d, s)
}

/// Structurizr's `filter`: the text without its non-word characters
/// (`(?U)\W`).
pub(crate) fn word(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect()
}

/// Standard base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Structurizr's `AbstractExporter.breakText`: the text unchanged when
/// shorter than `max` characters (`width / (font * 0.6)`), else words on
/// lines shorter than that, joined with `<br />`.
pub(crate) fn break_text(width: i32, font: u32, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    #[allow(clippy::cast_possible_truncation)] // a character count
    let max = (f64::from(width) / (f64::from(font) * 0.6)) as i64;
    let len = |s: &str| i64::try_from(s.encode_utf16().count()).unwrap_or(i64::MAX);
    if len(text) < max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut line: Option<String> = None;
    for word in text.split(' ') {
        match &mut line {
            None => line = Some(word.to_owned()),
            Some(l) => {
                if len(l) + len(word) + 1 < max {
                    l.push(' ');
                    l.push_str(word);
                } else {
                    out.push_str(l);
                    out.push_str("<br />");
                    word.clone_into(l);
                }
            }
        }
    }
    if let Some(l) = line {
        out.push_str(&l);
    }
    out
}

/// An icon as Structurizr holds it: a local file read becomes a `data:`
/// URI of its bytes; a written `data:` URI or URL stays as written.
pub(crate) fn icon_uri(model: &Model, written: &str) -> String {
    if written.starts_with("data:") {
        return written.to_owned();
    }
    match model.assets.iter().find(|a| a.path == written) {
        Some(a) => {
            let mime = match a.mime {
                crate::model::Mime::Png => "image/png",
                crate::model::Mime::Jpeg => "image/jpeg",
                _ => "image/svg+xml",
            };
            format!("data:{mime};base64,{}", base64(&a.bytes))
        }
        None => written.to_owned(),
    }
}

/// Structurizr's `IndentingWriter`: two spaces per level, a level below
/// zero writes at column zero (Structurizr's Ilograph exporter outdents
/// past it), lines joined with `\n`.
#[derive(Debug, Default)]
pub(crate) struct Writer {
    lines: Vec<String>,
    level: i32,
}

impl Writer {
    /// One line at the current level; a line break inside `text` is
    /// written `\n`, as Structurizr's writer does.
    pub(crate) fn line(&mut self, text: &str) {
        let n = usize::try_from(self.level.max(0)).unwrap_or(0) * 2;
        self.lines.push(format!("{}{}", " ".repeat(n), text.replace('\n', "\\n")));
    }

    pub(crate) fn blank(&mut self) {
        self.lines.push(String::new());
    }

    pub(crate) fn indent(&mut self) {
        self.level += 1;
    }

    pub(crate) fn outdent(&mut self) {
        self.level -= 1;
    }

    /// The text, with the final newline P3-403 asks for.
    pub(crate) fn finish(self) -> String {
        let mut s = self.lines.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s
    }
}
