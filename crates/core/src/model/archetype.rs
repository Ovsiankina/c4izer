//! `archetypes { ... }` (P2-155..P2-158, P2-181): named defaults for
//! elements and relationships, kept in the builder; the model only sees
//! the elements and relationships declared from them.

// DECISION: private submodule of `model` (plan Global Constraints), an
// `impl` block on the builder.

use super::{
    Builder, ElementKind, Head, Literal, ModelError, ModelErrorKind, Perspective, Property,
    declared_at, element_kind,
};
use crate::text::{Statement, TokenKind};

/// What an archetype declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Base {
    /// An element kind the model builds.
    Element(ElementKind),
    /// `group`.
    Group,
    /// `deploymentNode` or `infrastructureNode`: walked, not built
    /// (P2-120); the lowercase keyword.
    Deployment(&'static str),
    /// `->` or `--<archetype>->`.
    Relationship,
}

/// One archetype, its base's defaults already merged in (P2-156).
#[derive(Debug, Clone)]
pub(super) struct Archetype {
    pub(super) base: Base,
    pub(super) description: Option<Literal>,
    pub(super) technology: Option<Literal>,
    pub(super) metadata: Option<Literal>,
    pub(super) url: Option<Literal>,
    /// The chain's tags, root archetype first (B dsl-breadth 15a).
    pub(super) tags: Vec<String>,
    pub(super) properties: Vec<Property>,
    pub(super) perspectives: Vec<Perspective>,
}

impl Archetype {
    pub(super) const fn new(base: Base) -> Archetype {
        Archetype {
            base,
            description: None,
            technology: None,
            metadata: None,
            url: None,
            tags: Vec::new(),
            properties: Vec::new(),
            perspectives: Vec::new(),
        }
    }
}

/// The base a keyword declares (P2-158), ignoring case.
fn keyword_base(word: &str) -> Option<Base> {
    match word.to_ascii_lowercase().as_str() {
        "group" => Some(Base::Group),
        "element" => Some(Base::Element(ElementKind::Custom)),
        "deploymentnode" => Some(Base::Deployment("deploymentnode")),
        "infrastructurenode" => Some(Base::Deployment("infrastructurenode")),
        _ => element_kind(word).map(Base::Element),
    }
}

/// The archetype name of an arrow word `--<name>->`, if it is one.
pub(super) fn arrow_archetype(word: &str) -> Option<&str> {
    word.strip_prefix("--")?
        .strip_suffix("->")
        .filter(|name| !name.is_empty())
}

impl Builder<'_> {
    /// `archetypes { <name> = <base> { ... } ... }` in `model`.
    pub(super) fn archetypes(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<(), ModelError> {
        self.no_arguments(head)?;
        for inner in self.block(statement, head)? {
            let head = self.head(inner)?;
            if self.common(inner, &head)? {
                continue;
            }
            self.archetype(inner, &head)?;
        }
        Ok(())
    }

    /// `<name> = <base> { ... }`, or `<keyword> { ... }` / `-> { ... }`:
    /// the default archetype of that keyword.
    // TODO(srs): P2-155..P2-158 name no unnamed archetype. Reading taken
    // (Structurizr 6.2.4 probed, corpus archetypes-for-defaults.dsl and
    // archetypes-for-extension.dsl): `softwareSystem { ... }` (any base
    // keyword) or `-> { ... }` replaces what that keyword declares from
    // then on, so later elements, relationships and archetypes based on
    // the keyword take its defaults; one declared before it does not.
    // DECISION: the default is kept under the lowercased keyword (`->`
    // for relationships), so the keyword lookup that finds a named
    // archetype finds it.
    fn archetype(&mut self, statement: &Statement, head: &Head<'_>) -> Result<(), ModelError> {
        let (Some(_), kw) = declared_at(head) else {
            if head.len() == 1 {
                let mut archetype = self.archetype_base(head, 0)?;
                let key = if head.kind(0) == Some(TokenKind::Arrow) {
                    "->".to_owned()
                } else if keyword_base(head.text(0)).is_some() {
                    head.text(0).to_ascii_lowercase()
                } else {
                    // A named archetype is no keyword to default.
                    return Err(self.reject(head, 0));
                };
                self.archetype_body(statement, &mut archetype)?;
                self.archetypes.insert(key, archetype);
                return Ok(());
            }
            return Err(self.reject(head, 0));
        };
        if head.kind(0) != Some(TokenKind::Word) || !super::is_identifier(head.text(0)) {
            return Err(self.invalid(head, "archetypes", 0));
        }
        if head.len() <= kw {
            return Err(self.missing(head, "base"));
        }
        if head.len() > kw + 1 {
            return Err(self.invalid(head, "archetypes", kw + 1));
        }
        let mut archetype = self.archetype_base(head, kw)?;
        self.archetype_body(statement, &mut archetype)?;
        self.archetypes
            .insert(head.text(0).to_ascii_lowercase(), archetype);
        Ok(())
    }

    /// The lines of an archetype's block, if it has one.
    fn archetype_body(
        &mut self,
        statement: &Statement,
        archetype: &mut Archetype,
    ) -> Result<(), ModelError> {
        if let Some(block) = &statement.block {
            for inner in &block.statements {
                let line = self.head(inner)?;
                if self.common(inner, &line)? {
                    continue;
                }
                self.archetype_line(inner, &line, archetype)?;
            }
        }
        Ok(())
    }

    /// The archetype a base word at token `kw` starts from: a fresh one
    /// for a keyword, a copy of a declared archetype (P2-156).
    fn archetype_base(&self, head: &Head<'_>, kw: usize) -> Result<Archetype, ModelError> {
        let word = head.text(kw);
        if head.kind(kw) == Some(TokenKind::Arrow) {
            return Ok(self
                .archetypes
                .get("->")
                .cloned()
                .unwrap_or_else(|| Archetype::new(Base::Relationship)));
        }
        if let Some(name) = arrow_archetype(word) {
            return match self.archetypes.get(&name.to_ascii_lowercase()) {
                Some(a) if a.base == Base::Relationship => Ok(a.clone()),
                _ => Err(self.error(
                    ModelErrorKind::UnknownArchetype { name: name.into() },
                    head.span(kw),
                )),
            };
        }
        let lower = word.to_ascii_lowercase();
        if let Some(base) = keyword_base(word) {
            // The keyword's default archetype, when one is declared.
            return Ok(self
                .archetypes
                .get(&lower)
                .filter(|a| a.base == base)
                .cloned()
                .unwrap_or_else(|| Archetype::new(base)));
        }
        match self.archetypes.get(&lower) {
            Some(a) if a.base != Base::Relationship => Ok(a.clone()),
            _ => Err(self.error(
                ModelErrorKind::InvalidArchetype { base: word.into() },
                head.span(kw),
            )),
        }
    }

    /// One line of an archetype body.
    // DECISION: Structurizr for Java (checked): every base takes
    // `description`, `tag`, `tags`, `properties` and `perspectives`;
    // `technology` only containers, components, deployment and
    // infrastructure nodes and relationships (B dsl-breadth 15c:
    // elsewhere an unknown construct), `metadata` only custom elements,
    // `url` every base.
    fn archetype_line(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        archetype: &mut Archetype,
    ) -> Result<(), ModelError> {
        let technology = matches!(
            archetype.base,
            Base::Element(ElementKind::Container | ElementKind::Component)
                | Base::Deployment(_)
                | Base::Relationship
        );
        let metadata = archetype.base == Base::Element(ElementKind::Custom);
        if head.is_keyword(0, "properties") {
            let properties = self.properties(statement, head)?;
            archetype.properties.extend(properties);
            return Ok(());
        }
        if head.is_keyword(0, "perspectives") {
            let perspectives = self.perspectives(statement, head)?;
            archetype.perspectives.extend(perspectives);
            return Ok(());
        }
        self.no_block(statement, head)?;
        if head.is_keyword(0, "tags") {
            let tags = self.tags_line(head)?;
            archetype
                .tags
                .extend(super::effective_tags(&[], &tags));
        } else if head.is_keyword(0, "tag") {
            let tag = self.one_string(head)?;
            archetype
                .tags
                .extend(super::effective_tags(&[], std::slice::from_ref(&tag)));
        } else if head.is_keyword(0, "description") {
            archetype.description = Some(self.one_string(head)?);
        } else if head.is_keyword(0, "technology") && technology {
            archetype.technology = Some(self.one_string(head)?);
        } else if head.is_keyword(0, "metadata") && metadata {
            archetype.metadata = Some(self.one_string(head)?);
        } else if head.is_keyword(0, "url") {
            archetype.url = Some(self.one_string(head)?);
        } else {
            return Err(self.reject(head, 0));
        }
        Ok(())
    }

    /// The element archetype named by the word at token `kw`, if any.
    pub(super) fn element_archetype(&self, head: &Head<'_>, kw: usize) -> Option<&Archetype> {
        if head.kind(kw) != Some(TokenKind::Word) {
            return None;
        }
        self.archetypes
            .get(&head.text(kw).to_ascii_lowercase())
            .filter(|a| a.base != Base::Relationship)
    }

    /// The relationship archetype of the arrow at token `arrow`: `None`
    /// for a plain `->`, `UnknownArchetype` for an unknown `--name->`.
    pub(super) fn relationship_archetype(
        &self,
        head: &Head<'_>,
        arrow: usize,
    ) -> Result<Option<Archetype>, ModelError> {
        let Some(name) = arrow_archetype(head.text(arrow)) else {
            // A plain `->` takes the default relationship archetype.
            return Ok(self.archetypes.get("->").cloned());
        };
        match self.archetypes.get(&name.to_ascii_lowercase()) {
            Some(a) if a.base == Base::Relationship => Ok(Some(a.clone())),
            _ => Err(self.error(
                ModelErrorKind::UnknownArchetype { name: name.into() },
                head.span(arrow),
            )),
        }
    }
}
