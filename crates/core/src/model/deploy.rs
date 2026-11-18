//! Deployment environments (P3-1, P3-100..P3-109): environments, nodes,
//! infrastructure nodes and instances become model elements of the four
//! deployment kinds (D2); relationships written in a deployment block are
//! kept, model relationships are copied between instances (P3-106) and
//! `-/>` removes one copy (P3-108, D1).

// DECISION: private submodule of `model` (plan Global Constraints), an
// `impl` block on the builder.
// DECISION: under flat identifiers a deployment id is global, as every
// element id; under `!identifiers hierarchical` it is the dotted path from
// the environment's id (P3-109, `Model::full_key`).
// DECISION: a deployment `group { }` block holds its statements as if
// written outside it; it draws no group boundary (TODO(srs) below).
// TODO(srs): P3-1, a `group` block in a deployment environment or node.
// Reading taken: its statements count as written in the enclosing block
// and no group boundary is drawn; Structurizr 6.2.4 draws a group around
// them (docs/deferred.md).

use super::{
    Builder, Element, ElementId, ElementKind, Environment, EnvironmentId, Head, Identifiers,
    Literal, ModelError, ModelErrorKind, Relationship, Spanned, declared_at, effective_tags,
    index_u32,
};
use crate::text::{Span, Statement, TokenKind};

/// The block a deployment statement is read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Environment,
    Node,
    Infrastructure,
    Instance,
}

impl Place {
    /// The statements Structurizr allows in this block (DSL 6.2.4,
    /// checked), lowercase; `tag` is added as everywhere else (P2-18).
    fn allows(self, word: &str) -> bool {
        let words: &[&str] = match self {
            Place::Environment => &["deploymentgroup", "deploymentnode", "group"],
            Place::Node => &[
                "containerinstance",
                "deploymentgroup",
                "deploymentnode",
                "description",
                "group",
                "infrastructurenode",
                "instanceof",
                "instances",
                "perspectives",
                "properties",
                "softwaresysteminstance",
                "tag",
                "tags",
                "technology",
                "url",
            ],
            Place::Infrastructure => &[
                "description",
                "perspectives",
                "properties",
                "tag",
                "tags",
                "technology",
                "url",
            ],
            Place::Instance => &[
                "description",
                "healthcheck",
                "perspectives",
                "properties",
                "tag",
                "tags",
                "url",
            ],
        };
        words.contains(&word)
    }
}

/// A `src -/> dst` line, resolved once the model block is read.
#[derive(Debug, Clone)]
pub(super) struct Removal {
    source: String,
    destination: String,
    context: Option<ElementId>,
    span: Span,
    environment: Option<EnvironmentId>,
    /// The description written after the ends, if any: only copies of a
    /// relationship with that description are removed.
    description: Option<String>,
}

/// The state of the deployment walk, kept in the builder.
#[derive(Debug, Clone, Default)]
pub(super) struct DeployState {
    /// The environment being read.
    environment: Option<EnvironmentId>,
    /// Deployment group ids (lowercased) → (environment, name).
    groups: Vec<(String, Option<EnvironmentId>, String)>,
    /// Indices of the relationships written in deployment blocks.
    written: Vec<usize>,
    /// The environment each of `written` was read in.
    written_in: Vec<Option<EnvironmentId>>,
    /// `-/>` lines.
    removals: Vec<Removal>,
    /// The relationships written inside a `-/>` block, each with the
    /// index of its removal in `removals` (description and technology
    /// default to the removed relationship's).
    removal_blocks: Vec<(usize, usize)>,
}

impl DeployState {
    /// The environment being read, if any.
    pub(super) fn environment(&self) -> Option<EnvironmentId> {
        self.environment
    }
}

impl Builder<'_> {
    /// `[<id> =] deploymentEnvironment <name> { ... }` in `model`
    /// (P3-100).
    pub(super) fn deployment_environment(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<(), ModelError> {
        let (key, kw) = declared_at(head);
        let mut strings = self.strings(head, head.text(kw), kw + 1, 1)?.into_iter();
        let Some(name) = strings.next() else {
            return Err(self.missing(head, "name"));
        };
        if let Some(first) = self.model.environments.iter().find(|e| e.name.value == name.value) {
            return Err(self.error(
                ModelErrorKind::DuplicateName {
                    name: name.value.clone(),
                    first: first.name.span,
                    first_position: self.position(first.name.span),
                },
                name.span,
            ));
        }
        let lower = key.map(|k| self.declare_other(head, k)).transpose()?;
        self.environments.push((name.value.clone(), lower));
        let key = key.map(|k| Spanned {
            value: head.text(k).to_owned(),
            span: head.span(k),
        });
        let id = EnvironmentId(index_u32(self.model.environments.len()));
        self.model.environments.push(Environment {
            id,
            key,
            name,
            span: statement.span,
        });
        let outer = self.deploy.environment.replace(id);
        let result = self.deployment_block(statement, Place::Environment, None);
        self.deploy.environment = outer;
        result
    }

    /// `!element <id>` naming a deployment environment: its block read as
    /// that environment's. False when `written` names none.
    // TODO(srs): P2-17 and P2-120 do not say `!element` reaches deployment
    // elements. Reading taken (Structurizr 6.2.4 opens corpus
    // deployment-environment-empty.dsl and find-element.dsl): it does, by
    // id or by canonical name (`DeploymentNode://Env/Node/...`,
    // `InfrastructureNode://...`), with the statements the owner's own
    // block allows.
    pub(super) fn deployment_element(&mut self, statement: &Statement, written: &str) -> Result<bool, ModelError> {
        let Some(environment) = self
            .model
            .environments
            .iter()
            .find(|e| e.key.as_ref().is_some_and(|k| k.value.eq_ignore_ascii_case(written)))
            .map(|e| e.id)
        else {
            return Ok(false);
        };
        let outer = self.deploy.environment.replace(environment);
        let result = self.deployment_block(statement, Place::Environment, None);
        self.deploy.environment = outer;
        result.map(|()| true)
    }

    /// `!element` naming the deployment element `id`: its block read as
    /// that element's own, in its environment.
    pub(super) fn reopen_deployment(&mut self, statement: &Statement, id: ElementId) -> Result<(), ModelError> {
        let element = self.model.element(id);
        let place = match element.kind {
            ElementKind::DeploymentNode => Place::Node,
            ElementKind::InfrastructureNode => Place::Infrastructure,
            _ => Place::Instance,
        };
        let outer = std::mem::replace(&mut self.deploy.environment, element.environment);
        let result = self.deployment_block(statement, place, Some(id));
        self.deploy.environment = outer;
        result
    }

    /// The statements of a deployment block whose owner is `owner` (the
    /// node, infrastructure node or instance; `None` for an environment).
    fn deployment_block(
        &mut self,
        statement: &Statement,
        place: Place,
        owner: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        self.each(&block.statements, &mut |this, inner| {
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            this.deployment_statement(inner, &head, place, owner)
        })
    }

    /// Registers the non-element deployment id at token `k` (an
    /// environment or deployment group); its lowercased text.
    fn declare_other(&mut self, head: &Head<'_>, k: usize) -> Result<String, ModelError> {
        if head.kind(k) != Some(TokenKind::Word) || !super::is_identifier(head.text(k)) {
            return Err(self.invalid(head, "=", k));
        }
        self.check_free(head.text(k), head.span(k))?;
        let lower = head.text(k).to_ascii_lowercase();
        self.other_ids.insert(lower.clone(), head.span(k));
        Ok(lower)
    }

    /// Registers the deployment group id at token `k`; its lowercased
    /// text. Under hierarchical ids the register key is scoped by the
    /// environment, so two environments may each declare it.
    // DECISION: as Structurizr's DeploymentElementParser looks a group up
    // under `<environment id>.<group id>` (corpus
    // deployment-groups-hierarchical.dsl, environments without ids); an
    // environment without an id scopes with a private token (`#` is no
    // identifier character, so no written id matches it).
    fn declare_group(&mut self, head: &Head<'_>, k: usize) -> Result<String, ModelError> {
        if self.model.identifiers != Identifiers::Hierarchical {
            return self.declare_other(head, k);
        }
        if head.kind(k) != Some(TokenKind::Word) || !super::is_identifier(head.text(k)) {
            return Err(self.invalid(head, "=", k));
        }
        let lower = head.text(k).to_ascii_lowercase();
        let scope = match self.deploy.environment {
            Some(e) => match &self.model.environments[e.0 as usize].key {
                Some(key) => key.value.to_ascii_lowercase(),
                None => format!("#environment{}", e.0),
            },
            None => "#".to_owned(),
        };
        let full = format!("{scope}.{lower}");
        self.check_free(&full, head.span(k))?;
        self.other_ids.insert(full, head.span(k));
        Ok(lower)
    }

    fn deployment_statement(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        place: Place,
        owner: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let (key, kw) = declared_at(head);
        let removal = (0..head.len()).any(|i| head.text(i) == "-/>");
        if removal {
            return self.removal(statement, head, owner);
        }
        if head.arrow().is_some() {
            let index = self.model.relationships.len();
            self.relationship(statement, head, owner, key)?;
            self.deploy.written.push(index);
            self.deploy.written_in.push(self.deploy.environment);
            return Ok(());
        }
        if head.len() <= kw {
            return Err(self.missing(head, "keyword"));
        }
        let mut word = head.text(kw).to_ascii_lowercase();
        // P2-158: an archetype based on a deployment or infrastructure node
        // stands for that keyword.
        let archetype = self.element_archetype(head, kw).cloned();
        if let Some(a) = &archetype
            && let super::archetype::Base::Deployment(keyword) = a.base
        {
            keyword.clone_into(&mut word);
        }
        if head.kind(kw) != Some(TokenKind::Word) || !place.allows(&word) {
            return Err(self.reject(head, kw));
        }
        let declares = matches!(
            word.as_str(),
            "deploymentgroup"
                | "deploymentnode"
                | "infrastructurenode"
                | "softwaresysteminstance"
                | "containerinstance"
                | "instanceof"
        );
        if key.is_some() && !declares {
            return Err(self.reject(head, kw));
        }
        match word.as_str() {
            "deploymentgroup" => {
                let name = self.deployment_strings(head, kw, 1)?.remove(0);
                self.no_block(statement, head)?;
                if let Some(k) = key {
                    let lower = self.declare_group(head, k)?;
                    let environment = self.deploy.environment;
                    self.deploy.groups.push((lower, environment, name.value));
                }
                Ok(())
            }
            "deploymentnode" | "infrastructurenode" => {
                self.node(statement, head, (key, kw), word == "deploymentnode", archetype, owner)
            }
            "softwaresysteminstance" | "containerinstance" | "instanceof" => {
                let id = self.instance(statement, head, key, kw, &word, owner)?;
                self.deployment_block(statement, Place::Instance, Some(id))
            }
            "group" => {
                self.deployment_strings(head, kw, 1)?;
                self.deployment_block(statement, place, owner)
            }
            _ => {
                let Some(owner) = owner else {
                    return Err(self.reject(head, kw));
                };
                self.deployment_line(statement, head, owner, &word)
            }
        }
    }

    /// `[<id> =] deploymentNode ...` (`node`) or `infrastructureNode ...`
    /// under `owner`, the id and keyword at `(key, kw)` (P3-101, P3-103).
    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        (key, kw): (Option<usize>, usize),
        node: bool,
        archetype: Option<super::archetype::Archetype>,
        owner: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let kind = if node {
            ElementKind::DeploymentNode
        } else {
            ElementKind::InfrastructureNode
        };
        // `deploymentNode <name> [description] [technology] [tags]
        // [instances]`, `infrastructureNode <name> [description]
        // [technology] [tags]`.
        let mut strings = self.deployment_strings(head, kw, if node { 5 } else { 4 })?.into_iter();
        let name = strings.next().unwrap_or_else(|| unreachable!("deployment_strings has a name"));
        let a = archetype.unwrap_or_else(|| {
            super::archetype::Archetype::new(super::archetype::Base::Deployment(if node {
                "deploymentnode"
            } else {
                "infrastructurenode"
            }))
        });
        let description = strings.next().or(a.description);
        let technology = strings.next().or(a.technology);
        let declared_tags: Vec<Literal> = strings.next().into_iter().collect();
        let instances = strings.next();
        if let Some(i) = &instances {
            self.check_instances(i)?;
        }
        let id = self.push_deployment_element(head, key, kind, owner, name, statement.span)?;
        if !a.tags.is_empty() {
            self.archetype_tags.insert(id, a.tags);
        }
        let e = &mut self.model.elements[id.0 as usize];
        e.description = description;
        e.technology = technology;
        e.declared_tags = declared_tags;
        e.instances = instances;
        e.url = a.url;
        e.properties = a.properties;
        e.perspectives = a.perspectives;
        let inner = if node { Place::Node } else { Place::Infrastructure };
        self.deployment_block(statement, inner, Some(id))?;
        self.retag_element(id);
        Ok(())
    }

    /// The name after the keyword at `kw` and at most `max - 1` more
    /// strings; the name is always there.
    fn deployment_strings(&self, head: &Head<'_>, kw: usize, max: usize) -> Result<Vec<Literal>, ModelError> {
        let strings = self.strings(head, head.text(kw), kw + 1, max)?;
        if strings.is_empty() {
            return Err(self.missing(head, "name"));
        }
        Ok(strings)
    }

    /// P3-102: a positive integer, or `m..n` with `0 < m <= n`.
    fn check_instances(&self, value: &Literal) -> Result<(), ModelError> {
        let number = |t: &str| t.parse::<u32>().ok().filter(|&n| n > 0);
        let ok = match value.value.split_once("..") {
            Some((m, n)) => matches!((number(m), number(n)), (Some(m), Some(n)) if m <= n),
            None => number(&value.value).is_some(),
        };
        if ok {
            Ok(())
        } else {
            Err(self.error(
                ModelErrorKind::InvalidValue {
                    construct: "instances".into(),
                    value: value.value.clone(),
                },
                value.span,
            ))
        }
    }

    /// Pushes a deployment element of `kind` under `parent` in the current
    /// environment, its id (at token `key`) registered (P3-109).
    // DECISION: plain arguments, as the builder's other declaration
    // helpers take them; a struct for one call site adds nothing.
    #[allow(clippy::too_many_arguments)]
    fn push_deployment_element(
        &mut self,
        head: &Head<'_>,
        key: Option<usize>,
        kind: ElementKind,
        parent: Option<ElementId>,
        name: Literal,
        span: Span,
    ) -> Result<ElementId, ModelError> {
        let key = match key {
            Some(k) => {
                if head.kind(k) != Some(TokenKind::Word) || !super::is_identifier(head.text(k)) {
                    return Err(self.invalid(head, kind.keyword(), k));
                }
                Some(Spanned {
                    value: head.text(k).to_owned(),
                    span: head.span(k),
                })
            }
            None => None,
        };
        let id = ElementId(index_u32(self.model.elements.len()));
        if let Some(k) = &key {
            let full = self.deployment_full_key(parent, &k.value);
            self.check_free(&full, k.span)?;
            self.keys.insert(full.to_ascii_lowercase(), id);
        }
        self.model.elements.push(Element {
            id,
            key,
            kind,
            parent,
            name,
            description: None,
            technology: None,
            tags: Vec::new(),
            declared_tags: Vec::new(),
            span,
            group: None,
            url: None,
            properties: Vec::new(),
            perspectives: Vec::new(),
            metadata: None,
            docs: Vec::new(),
            environment: self.deploy.environment,
            instance_of: None,
            instance_number: None,
            instances: None,
            deployment_groups: Vec::new(),
        });
        Ok(id)
    }

    /// The register key of deployment id `key` declared under `parent`:
    /// `key` under flat ids; under hierarchical ids the parent's full id
    /// (or, at the top, the environment's id) then `.key` (P3-109).
    fn deployment_full_key(&self, parent: Option<ElementId>, key: &str) -> String {
        if self.model.identifiers != Identifiers::Hierarchical {
            return key.to_owned();
        }
        let prefix = match parent {
            Some(p) => self.model.full_key(p),
            None => self
                .deploy
                .environment
                .and_then(|e| self.model.environments[e.0 as usize].key.as_ref())
                .map(|k| k.value.clone()),
        };
        match prefix {
            Some(prefix) => format!("{prefix}.{key}"),
            None => key.to_owned(),
        }
    }

    /// `softwareSystemInstance <id> [groups] [tags]`, `containerInstance
    /// ...`, `instanceOf <id> ...` (P3-104): an instance of a declared
    /// element of the right kind, numbered per element per environment.
    #[allow(clippy::too_many_arguments)]
    fn instance(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        key: Option<usize>,
        kw: usize,
        word: &str,
        owner: Option<ElementId>,
    ) -> Result<ElementId, ModelError> {
        if head.len() <= kw + 1 {
            return Err(self.missing(head, "element"));
        }
        let element = self.view_element(head, kw + 1)?;
        let of = self.model.element(element.value);
        let kind = match (word, of.kind) {
            ("softwaresysteminstance" | "instanceof", ElementKind::SoftwareSystem) => {
                ElementKind::SoftwareSystemInstance
            }
            ("containerinstance" | "instanceof", ElementKind::Container) => ElementKind::ContainerInstance,
            _ => return Err(self.invalid(head, head.text(kw), kw + 1)),
        };
        let name = of.name.clone();
        let mut strings = self.strings(head, head.text(kw), kw + 2, 2)?.into_iter();
        let groups = match strings.next() {
            Some(g) => self.group_names(&g),
            None => Vec::new(),
        };
        let declared_tags: Vec<Literal> = strings.collect();
        let environment = self.deploy.environment;
        let number = self
            .model
            .elements
            .iter()
            .filter(|e| e.instance_of == Some(element.value) && e.environment == environment)
            .count()
            + 1;
        let id = self.push_deployment_element(head, key, kind, owner, name, statement.span)?;
        let e = &mut self.model.elements[id.0 as usize];
        e.instance_of = Some(element.value);
        e.instance_number = Some(index_u32(number));
        e.deployment_groups = groups;
        e.declared_tags = declared_tags;
        Ok(id)
    }

    /// The deployment group names a `"g1,g2"` argument names, each a
    /// declared deployment group id of the current environment.
    // DECISION: a deployment group id is looked up ignoring case, as every
    // id; an unknown one is ignored with a no-effect notice, as
    // Structurizr's DeploymentElementParser does ("backwards
    // compatibility"; corpus big-bank-plc.dsl writes
    // `containerInstance database "Failover"` with no such group).
    fn group_names(&mut self, literal: &Literal) -> Vec<String> {
        let mut out = Vec::new();
        for part in literal.value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let lower = part.to_ascii_lowercase();
            let found = self
                .deploy
                .groups
                .iter()
                .find(|(id, env, _)| *id == lower && *env == self.deploy.environment)
                .map(|(_, _, name)| name.clone());
            match found {
                Some(name) => {
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
                None => {
                    self.notice(&format!("deployment group `{part}`"), None, literal.span);
                }
            }
        }
        out
    }

    /// A body line of a node, infrastructure node or instance block,
    /// stored on `owner`.
    fn deployment_line(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        owner: ElementId,
        word: &str,
    ) -> Result<(), ModelError> {
        let i = owner.0 as usize;
        match word {
            "healthcheck" => {
                // `healthCheck <name> <url> [interval] [timeout]`.
                if head.len() < 3 {
                    return Err(self.missing(head, "name and URL"));
                }
                self.strings(head, "healthCheck", 1, 4)?;
                self.no_block(statement, head)?;
                self.notice("healthCheck", None, head.span_of(0, head.len() - 1));
            }
            "instances" => {
                let value = self.one_word(head)?;
                self.no_block(statement, head)?;
                let literal = head.literal(1);
                debug_assert_eq!(literal.value, value);
                self.check_instances(&literal)?;
                self.model.elements[i].instances = Some(literal);
            }
            "tags" => {
                let tags = self.tags_line(head)?;
                self.no_block(statement, head)?;
                self.model.elements[i].declared_tags.extend(tags);
                self.retag_element(owner);
            }
            "tag" => {
                let tag = self.one_string(head)?;
                self.no_block(statement, head)?;
                self.model.elements[i].declared_tags.push(tag);
                self.retag_element(owner);
            }
            "properties" => {
                let properties = self.properties(statement, head)?;
                self.model.elements[i].properties.extend(properties);
            }
            "perspectives" => {
                let perspectives = self.perspectives(statement, head)?;
                self.model.elements[i].perspectives.extend(perspectives);
            }
            "description" => {
                let value = self.one_string(head)?;
                self.no_block(statement, head)?;
                self.model.elements[i].description = Some(value);
            }
            "technology" => {
                let value = self.one_string(head)?;
                self.no_block(statement, head)?;
                self.model.elements[i].technology = Some(value);
            }
            // `url`.
            _ => {
                let value = self.one_string(head)?;
                self.no_block(statement, head)?;
                self.model.elements[i].url = Some(value);
            }
        }
        Ok(())
    }

    /// `src -/> dst` in a deployment block (P3-108, D1): resolved after
    /// the model block.
    // TODO(srs): P3-108 names no block after `a -/> b`, and no software
    // system or container as an end. Reading taken (Structurizr 6.2.4,
    // NoRelationshipParser and corpus no-relationship.dsl): an end naming
    // a system or container stands for all its instances in the
    // environment, and a `{ }` block after the line holds `->` lines read
    // as written in the environment, each taking the removed
    // relationship's description and technology unless it writes its own.
    fn removal(&mut self, statement: &Statement, head: &Head<'_>, owner: Option<ElementId>) -> Result<(), ModelError> {
        let arrow = (0..head.len()).find(|&i| head.text(i) == "-/>").unwrap_or_default();
        if arrow != 1 || head.len() < 3 {
            return Err(self.missing(head, "source and destination"));
        }
        for i in [0, 2] {
            if head.kind(i) != Some(TokenKind::Word) {
                return Err(self.invalid(head, "-/>", i));
            }
        }
        // DECISION: a `-/>` line may name the description of the
        // relationship whose copy it removes (the language reference's
        // `a -/> b "description"`); without one it removes every copy
        // between the two instances.
        let description = self.strings(head, "-/>", 3, 1)?.into_iter().next().map(|d| d.value);
        self.deploy.removals.push(Removal {
            description,
            source: head.text(0).to_owned(),
            destination: head.text(2).to_owned(),
            context: owner,
            span: head.span_of(0, 2),
            environment: self.deploy.environment,
        });
        let removal = self.deploy.removals.len() - 1;
        let Some(block) = &statement.block else {
            return Ok(());
        };
        self.each(&block.statements, &mut |this, inner| {
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            if head.arrow().is_none() || (0..head.len()).any(|i| head.text(i) == "-/>") {
                return Err(this.reject(&head, 0));
            }
            let (key, _) = declared_at(&head);
            let index = this.model.relationships.len();
            this.relationship(inner, &head, owner, key)?;
            this.deploy.written.push(index);
            this.deploy.written_in.push(this.deploy.environment);
            this.deploy.removal_blocks.push((index, removal));
            Ok(())
        })
    }

    /// After the model block: instance tags (P3-105), the written
    /// deployment relationships checked (P3-107), the copied relationships
    /// added (P3-106) less the `-/>` removals (P3-108).
    pub(super) fn finish_deployment(&mut self) -> Result<(), ModelError> {
        self.retag_instances();
        self.removal_block_defaults()?;
        self.expand_written();
        for &index in &self.deploy.written {
            let r = &self.model.relationships[index];
            let instance = |e: ElementId| {
                matches!(
                    self.model.element(e).kind,
                    ElementKind::SoftwareSystemInstance | ElementKind::ContainerInstance
                )
            };
            if instance(r.source.value) && instance(r.destination.value) {
                return Err(self.error(
                    ModelErrorKind::DeploymentRelationship {
                        source: self.written(r.source.span).to_owned(),
                        destination: self.written(r.destination.span).to_owned(),
                        reason: "not permitted",
                    },
                    r.span,
                ));
            }
        }
        let mut copies = self.copies();
        for removal in std::mem::take(&mut self.deploy.removals) {
            let source = self.removal_end(&removal, &removal.source)?;
            let destination = self.removal_end(&removal, &removal.destination)?;
            let before = copies.len();
            let model = &self.model;
            copies.retain(|(r, s, d)| {
                let named = removal.description.as_ref().is_none_or(|text| {
                    model.relationship(*r).description.as_ref().is_some_and(|x| x.value == *text)
                });
                !(source.contains(s) && destination.contains(d) && named)
            });
            if copies.len() == before {
                return Err(self.error(
                    ModelErrorKind::DeploymentRelationship {
                        source: removal.source.clone(),
                        destination: removal.destination.clone(),
                        reason: "no copied relationship",
                    },
                    removal.span,
                ));
            }
        }
        for (original, source, destination) in copies {
            let r = self.model.relationship(original).clone();
            let id = super::RelationshipId(index_u32(self.model.relationships.len()));
            self.model.relationships.push(Relationship {
                id,
                key: None,
                source: Spanned {
                    value: source,
                    span: r.source.span,
                },
                destination: Spanned {
                    value: destination,
                    span: r.destination.span,
                },
                copied_from: Some(original),
                ..r
            });
        }
        Ok(())
    }

    /// One end of a removal: an instance of the removal's environment, or
    /// a software system or container standing for all its instances
    /// there.
    fn removal_end(&self, removal: &Removal, written: &str) -> Result<Vec<ElementId>, ModelError> {
        let found = self.resolve_in(written, removal.context, removal.environment).and_then(|e| {
            let element = self.model.element(e);
            match element.kind {
                ElementKind::SoftwareSystemInstance | ElementKind::ContainerInstance
                    if element.environment == removal.environment =>
                {
                    Some(vec![e])
                }
                ElementKind::SoftwareSystem | ElementKind::Container => {
                    Some(self.instances_in(e, removal.environment))
                }
                _ => None,
            }
        });
        found.ok_or_else(|| self.error(ModelErrorKind::UndeclaredId { id: written.to_owned() }, removal.span))
    }

    /// The instances of `element` in `environment`, in declaration order.
    fn instances_in(&self, element: ElementId, environment: Option<EnvironmentId>) -> Vec<ElementId> {
        self.model
            .elements
            .iter()
            .filter(|x| x.instance_of == Some(element) && x.environment == environment)
            .map(|x| x.id)
            .collect()
    }

    /// The relationships written in a `-/>` block take the removed
    /// relationship's description and technology where they write none.
    fn removal_block_defaults(&mut self) -> Result<(), ModelError> {
        for (index, r) in std::mem::take(&mut self.deploy.removal_blocks) {
            let removal = self.deploy.removals[r].clone();
            let statics = |this: &Self, ends: Vec<ElementId>| -> Vec<ElementId> {
                ends.into_iter().map(|e| this.model.element(e).instance_of.unwrap_or(e)).collect()
            };
            let sources = statics(self, self.removal_end(&removal, &removal.source)?);
            let destinations = statics(self, self.removal_end(&removal, &removal.destination)?);
            let removed = self.model.relationships.iter().find(|m| {
                m.copied_from.is_none()
                    && !self.deploy.written.contains(&(m.id.0 as usize))
                    && sources.contains(&m.source.value)
                    && destinations.contains(&m.destination.value)
                    && removal
                        .description
                        .as_ref()
                        .is_none_or(|d| m.description.as_ref().is_some_and(|x| x.value == *d))
            });
            let (description, technology) = match removed {
                Some(m) => (m.description.clone(), m.technology.clone()),
                None => continue,
            };
            let r = &mut self.model.relationships[index];
            if r.description.is_none() {
                r.description = description;
            }
            if r.technology.is_none() {
                r.technology = technology;
            }
        }
        Ok(())
    }

    /// A relationship written in a deployment block with a software system
    /// or container end stands for one relationship per instance of it in
    /// that environment (Structurizr `ExplicitRelationshipParser`).
    // DECISION: when an end has no instance in the environment the
    // relationship is kept as written (Structurizr creates none); removing
    // it would renumber the relationships after it.
    fn expand_written(&mut self) {
        let written: Vec<(usize, Option<EnvironmentId>)> =
            self.deploy.written.iter().copied().zip(self.deploy.written_in.iter().copied()).collect();
        for (index, environment) in written {
            if environment.is_none() {
                continue;
            }
            let r = self.model.relationships[index].clone();
            let ends = |this: &Self, e: ElementId| match this.model.element(e).kind {
                ElementKind::SoftwareSystem | ElementKind::Container => this.instances_in(e, environment),
                _ => vec![e],
            };
            let sources = ends(self, r.source.value);
            let destinations = ends(self, r.destination.value);
            if sources.is_empty() || destinations.is_empty() {
                continue;
            }
            let mut pairs = sources.iter().flat_map(|&s| destinations.iter().map(move |&d| (s, d)));
            let Some((s, d)) = pairs.next() else { continue };
            let rest: Vec<(ElementId, ElementId)> = pairs.collect();
            self.model.relationships[index].source.value = s;
            self.model.relationships[index].destination.value = d;
            for (s, d) in rest {
                let id = super::RelationshipId(index_u32(self.model.relationships.len()));
                self.deploy.written.push(id.0 as usize);
                self.deploy.written_in.push(environment);
                self.model.relationships.push(Relationship {
                    id,
                    key: None,
                    source: Spanned { value: s, span: r.source.span },
                    destination: Spanned { value: d, span: r.destination.span },
                    ..r.clone()
                });
            }
        }
    }

    /// Every (model relationship, source instance, destination instance)
    /// to copy (P3-106), in relationship order then instance order.
    // DECISION: a relationship end that is a component is lifted to its
    // container (Structurizr's default implied-relationships strategy)
    // before its instances are looked up; a lifted pair that has a
    // relationship of its own, or was lifted before, is copied once (the
    // direct relationship wins, else the first declared). Only components
    // are lifted, never containers to their system.
    fn copies(&self) -> Vec<(super::RelationshipId, ElementId, ElementId)> {
        let instances_of = |e: ElementId| -> Vec<&Element> {
            self.model.elements.iter().filter(|x| x.instance_of == Some(e)).collect()
        };
        let lift = |e: ElementId| -> ElementId {
            let element = self.model.element(e);
            match (element.kind, element.parent) {
                (ElementKind::Component, Some(parent)) if instances_of(e).is_empty() => parent,
                _ => e,
            }
        };
        let mut seen: Vec<(ElementId, ElementId)> = self
            .model
            .relationships
            .iter()
            .filter(|r| r.copied_from.is_none() && !self.deploy.written.contains(&(r.id.0 as usize)))
            .filter(|r| lift(r.source.value) == r.source.value && lift(r.destination.value) == r.destination.value)
            .map(|r| (r.source.value, r.destination.value))
            .collect();
        let shares_group = |a: &Element, b: &Element| {
            if a.deployment_groups.is_empty() || b.deployment_groups.is_empty() {
                return a.deployment_groups.is_empty() && b.deployment_groups.is_empty();
            }
            a.deployment_groups.iter().any(|g| b.deployment_groups.contains(g))
        };
        let mut out = Vec::new();
        for r in &self.model.relationships {
            if r.copied_from.is_some() || self.deploy.written.contains(&(r.id.0 as usize)) {
                continue;
            }
            let (source, destination) = (lift(r.source.value), lift(r.destination.value));
            let lifted = (source, destination) != (r.source.value, r.destination.value);
            if lifted {
                if source == destination || seen.contains(&(source, destination)) {
                    continue;
                }
                seen.push((source, destination));
            }
            let sources = instances_of(source);
            if sources.is_empty() {
                continue;
            }
            let destinations = instances_of(destination);
            for s in &sources {
                for d in &destinations {
                    if s.id != d.id && s.environment == d.environment && shares_group(s, d) {
                        out.push((r.id, s.id, d.id));
                    }
                }
            }
        }
        out
    }

    /// P3-105: an instance's tags are its element's tags, then its kind
    /// tag, then its own declared tags (Structurizr
    /// `getAllTagsForElement`).
    fn retag_instances(&mut self) {
        for i in 0..self.model.elements.len() {
            let Some(of) = self.model.elements[i].instance_of else {
                continue;
            };
            let base: Vec<String> = self.model.element(of).tags.clone();
            let e = &self.model.elements[i];
            let mut implicit: Vec<&str> = base.iter().map(String::as_str).collect();
            implicit.push(e.kind.default_tag());
            let tags = effective_tags(&implicit, &e.declared_tags);
            self.model.elements[i].tags = tags;
        }
    }
}
