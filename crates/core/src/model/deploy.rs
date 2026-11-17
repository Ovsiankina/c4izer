//! Deployment environments (P2-120): walked and id-checked, nothing built.
//! Phase 3 draws deployment views; until then a deployment id only joins
//! the id register and a reference must name a declared id.

// DECISION: private submodule of `model` (plan Global Constraints), an
// `impl` block on the builder.
// DECISION: deployment ids stay flat (global) under `!identifiers
// hierarchical`: nothing addresses them by a full id before phase 3.

use super::{Builder, ElementKind, ModelError, ModelErrorKind, declared_at};
use crate::text::{Statement, TokenKind};

use super::Head;

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

impl Builder<'_> {
    /// `[<id> =] deploymentEnvironment <name> { ... }` in `model`.
    pub(super) fn deployment_environment(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
    ) -> Result<(), ModelError> {
        let (key, kw) = declared_at(head);
        let name = self.deployment_name(head, kw, 1)?;
        let key = match key {
            Some(k) => Some(self.declare_other(head, k)?),
            None => None,
        };
        self.environments.push((name, key));
        self.deployment_block(statement, Place::Environment)
    }

    /// The statements of a deployment block.
    fn deployment_block(&mut self, statement: &Statement, place: Place) -> Result<(), ModelError> {
        let Some(block) = &statement.block else {
            return Ok(());
        };
        self.each(&block.statements, &mut |this, inner| {
            let head = this.head(inner)?;
            if this.common(inner, &head)? {
                return Ok(());
            }
            this.deployment_statement(inner, &head, place)?;
            Ok(())
        })?;
        Ok(())
    }

    /// The name after the keyword at `kw` and at most `extra` more strings.
    fn deployment_name(
        &self,
        head: &Head<'_>,
        kw: usize,
        max: usize,
    ) -> Result<String, ModelError> {
        let strings = self.strings(head, head.text(kw), kw + 1, max)?;
        match strings.into_iter().next() {
            Some(name) => Ok(name.value),
            None => Err(self.missing(head, "name")),
        }
    }

    /// Registers the deployment id at token `k`; its lowercased text.
    fn declare_other(&mut self, head: &Head<'_>, k: usize) -> Result<String, ModelError> {
        if head.kind(k) != Some(TokenKind::Word) || !super::is_identifier(head.text(k)) {
            return Err(self.invalid(head, "=", k));
        }
        self.check_free(head.text(k), head.span(k))?;
        let lower = head.text(k).to_ascii_lowercase();
        self.other_ids.insert(lower.clone(), head.span(k));
        Ok(lower)
    }

    fn deployment_statement(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        place: Place,
    ) -> Result<(), ModelError> {
        let (key, kw) = declared_at(head);
        let removal = (0..head.len()).any(|i| head.text(i) == "-/>");
        if head.arrow().is_some() || removal {
            return self.deployment_relationship(statement, head, key);
        }
        if head.len() <= kw {
            return Err(self.missing(head, "keyword"));
        }
        let mut word = head.text(kw).to_ascii_lowercase();
        // P2-158: an archetype based on a deployment or infrastructure node
        // stands for that keyword.
        if let Some(a) = self.element_archetype(head, kw)
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
                self.deployment_name(head, kw, 1)?;
                self.no_block(statement, head)?;
            }
            // `deploymentNode <name> [description] [technology] [tags]
            // [instances]`.
            "deploymentnode" => self.deployment_name(head, kw, 5).map(drop)?,
            "infrastructurenode" => self.deployment_name(head, kw, 4).map(drop)?,
            "softwaresysteminstance" | "containerinstance" | "instanceof" => {
                self.instance(head, kw, &word)?;
            }
            "group" => {
                self.deployment_name(head, kw, 1)?;
                return self.deployment_block(statement, place);
            }
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
                self.one_word(head)?;
                self.no_block(statement, head)?;
            }
            "tags" => {
                self.tags_line(head)?;
                self.no_block(statement, head)?;
            }
            "properties" => return self.properties(statement, head).map(drop),
            "perspectives" => return self.perspectives(statement, head).map(drop),
            // `description`, `technology`, `url`, `tag`.
            _ => {
                self.one_string(head)?;
                self.no_block(statement, head)?;
            }
        }
        if let Some(k) = key {
            self.declare_other(head, k)?;
        }
        let inner = match word.as_str() {
            "deploymentnode" => Place::Node,
            "infrastructurenode" => Place::Infrastructure,
            _ => Place::Instance,
        };
        if statement.block.is_some() {
            if matches!(word.as_str(), "deploymentnode" | "infrastructurenode") || declares {
                return self.deployment_block(statement, inner);
            }
            return self.no_block(statement, head);
        }
        Ok(())
    }

    /// `softwareSystemInstance <id> [groups] [tags]`, `containerInstance
    /// ...`, `instanceOf <id> ...`: the id names a declared element of the
    /// right kind.
    fn instance(&self, head: &Head<'_>, kw: usize, word: &str) -> Result<(), ModelError> {
        if head.len() <= kw + 1 {
            return Err(self.missing(head, "element"));
        }
        let element = self.view_element(head, kw + 1)?;
        let kind = self.model.element(element.value).kind;
        let fits = match word {
            "softwaresysteminstance" => kind == ElementKind::SoftwareSystem,
            "containerinstance" => kind == ElementKind::Container,
            _ => matches!(kind, ElementKind::SoftwareSystem | ElementKind::Container),
        };
        if !fits {
            return Err(self.invalid(head, head.text(kw), kw + 1));
        }
        self.strings(head, head.text(kw), kw + 2, 2)?;
        Ok(())
    }

    /// `[<id> =] [src] -> dst ...` or `src -/> dst` in a deployment block:
    /// both ends name a declared element or deployment id.
    fn deployment_relationship(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        key: Option<usize>,
    ) -> Result<(), ModelError> {
        let from = key.map_or(0, |k| k + 2);
        let arrow = (0..head.len())
            .find(|&i| head.kind(i) == Some(TokenKind::Arrow) || head.text(i) == "-/>")
            .unwrap_or_default();
        if arrow < from || arrow > from + 1 {
            return Err(self.invalid(head, head.text(arrow), from));
        }
        if head.len() <= arrow + 1 {
            return Err(self.missing(head, "destination"));
        }
        let ends = if arrow == from {
            vec![arrow + 1]
        } else {
            vec![from, arrow + 1]
        };
        for i in ends {
            let t = head.text(i);
            if head.kind(i) != Some(TokenKind::Word) {
                return Err(self.invalid(head, "->", i));
            }
            let known = t.eq_ignore_ascii_case("this")
                || self.resolve(t, None).is_some()
                || self.other_ids.contains_key(&t.to_ascii_lowercase());
            if !known {
                return Err(self.error(ModelErrorKind::UndeclaredId { id: t.into() }, head.span(i)));
            }
        }
        self.strings(head, "->", arrow + 2, 3)?;
        if let Some(k) = key {
            self.declare_other(head, k)?;
        }
        // The block (`tags`, `url`, ...) is kept byte for byte.
        let _ = statement;
        Ok(())
    }
}
