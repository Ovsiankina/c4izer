//! Expressions (/dsl/expressions): parsing a written expression into
//! `Expression`, evaluating it (`Model::select_elements`,
//! `Model::select_relationships`), and the `!elements` / `!relationships`
//! directives (P2-166, P2-168).

// DECISION: private submodule of `model` (plan Global Constraints).
// DECISION: Structurizr for Java (DSL 6.2.4, checked form by form): an
// expression holding `&&` is split on `&&` and each side read whole (an
// `||` inside a side is part of its value); otherwise one holding `||` is
// split on `||`. So a mixed expression is accepted and evaluated as Java
// evaluates it, and `MixedOperators` is never raised (owner ruling,
// phase2-decisions Part C).
// DECISION: `->x`, `x->` and `->x->` count a relationship between
// descendants as one between their ancestors, as Structurizr's implied
// relationships do, unless `!impliedRelationships false`; `a->b` matches
// relationships written between `a` and `b` only (c4rizer builds no
// implied relationship objects).

use std::collections::BTreeSet;

use super::{
    Builder, ElementId, ElementKind, Expression, GroupId, Head, ImpliedRelationships, Model,
    ModelError, ModelErrorKind, RelationshipId, Span,
};
use crate::text::Statement;

/// Which items an expression selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Items {
    Elements,
    Relationships,
}

impl Model {
    /// True when `element` is `ancestor` or inside it.
    fn within(&self, element: ElementId, ancestor: ElementId) -> bool {
        element == ancestor || self.is_inside(element, ancestor)
    }

    /// The relationships whose ends are resolved (forward references not
    /// yet resolved are skipped while the model is being built).
    fn resolved_relationships(&self) -> impl Iterator<Item = &super::Relationship> {
        let n = self.elements.len();
        self.relationships
            .iter()
            .filter(move |r| (r.source.value.0 as usize) < n && (r.destination.value.0 as usize) < n)
    }

    /// True when `from` has a relationship to `to`: written between them,
    /// or (implied) between their descendants when neither holds the
    /// other.
    fn linked(&self, from: ElementId, to: ElementId) -> bool {
        let nested = self.within(from, to) || self.within(to, from);
        self.resolved_relationships().any(|r| {
            let (s, d) = (r.source.value, r.destination.value);
            // P2-7: per relationship, the strategy it was declared under.
            let implied = !nested && r.implied_relationships != ImpliedRelationships::False;
            (s == from && d == to) || (implied && self.within(s, from) && self.within(d, to))
        })
    }

    /// The members of `group` and of the groups nested in it.
    fn group_members(&self, group: GroupId) -> BTreeSet<ElementId> {
        let inside = |mut g: Option<GroupId>| {
            while let Some(id) = g {
                if id == group {
                    return true;
                }
                g = self.groups.get(id.0 as usize).and_then(|x| x.parent);
            }
            false
        };
        self.elements
            .iter()
            .filter(|e| inside(e.group))
            .map(|e| e.id)
            .collect()
    }

    fn eval_elements(&self, expression: &Expression) -> BTreeSet<ElementId> {
        let all = || self.elements.iter().map(|e| e.id);
        let has_all = |tags: &[String], have: &[String]| tags.iter().all(|t| have.contains(t));
        match expression {
            Expression::Element(id) => BTreeSet::from([*id]),
            Expression::Group(g) => self.group_members(*g),
            Expression::Afferent(inner) | Expression::Efferent(inner) | Expression::AfferentEfferent(inner) => {
                let base = self.eval_elements(inner);
                let to = !matches!(expression, Expression::Efferent(_));
                let from = !matches!(expression, Expression::Afferent(_));
                let mut out = base.clone();
                for e in all() {
                    let hit = base.iter().any(|&x| (to && self.linked(e, x)) || (from && self.linked(x, e)));
                    if hit {
                        out.insert(e);
                    }
                }
                out
            }
            Expression::ElementType { kind, .. } => match kind {
                Some(k) => all().filter(|&e| self.element(e).kind == *k).collect(),
                None => BTreeSet::new(),
            },
            Expression::ElementParent(p) => all().filter(|&e| self.element(e).parent == Some(*p)).collect(),
            Expression::ElementTag { tags, negated } => all()
                .filter(|&e| has_all(tags, &self.element(e).tags) != *negated)
                .collect(),
            Expression::ElementTechnology { technology, negated } => all()
                .filter(|&e| {
                    let element = self.element(e);
                    if !matches!(element.kind, ElementKind::Container | ElementKind::Component) {
                        return false;
                    }
                    let tech = element.technology.as_ref().map_or("", |t| t.value.as_str());
                    (tech == technology) != *negated
                })
                .collect(),
            Expression::ElementProperty { name, value } => all()
                .filter(|&e| {
                    self.element(e)
                        .properties
                        .iter()
                        .any(|p| p.name.value == *name && p.value.value == *value)
                })
                .collect(),
            Expression::ElementGroup(name) => all()
                .filter(|&e| {
                    self.element(e)
                        .group
                        .and_then(|g| self.groups.get(g.0 as usize))
                        .is_some_and(|g| g.full_name == *name)
                })
                .collect(),
            Expression::And(parts) => intersect(parts.iter().map(|p| self.eval_elements(p))),
            Expression::Or(parts) => parts.iter().flat_map(|p| self.eval_elements(p)).collect(),
            Expression::Relationships { .. }
            | Expression::RelationshipTag { .. }
            | Expression::RelationshipProperty { .. } => BTreeSet::new(),
        }
    }

    fn eval_relationships(&self, expression: &Expression) -> BTreeSet<RelationshipId> {
        let resolved = || self.resolved_relationships();
        match expression {
            Expression::Relationships {
                source,
                destination,
            } => resolved()
                .filter(|r| {
                    source.is_none_or(|s| r.source.value == s)
                        && destination.is_none_or(|d| r.destination.value == d)
                })
                .map(|r| r.id)
                .collect(),
            Expression::RelationshipTag { tags, negated } => resolved()
                .filter(|r| tags.iter().all(|t| r.tags.contains(t)) != *negated)
                .map(|r| r.id)
                .collect(),
            Expression::RelationshipProperty { name, value } => resolved()
                .filter(|r| {
                    r.properties
                        .iter()
                        .any(|p| p.name.value == *name && p.value.value == *value)
                })
                .map(|r| r.id)
                .collect(),
            Expression::And(parts) => intersect(parts.iter().map(|p| self.eval_relationships(p))),
            Expression::Or(parts) => parts.iter().flat_map(|p| self.eval_relationships(p)).collect(),
            _ => BTreeSet::new(),
        }
    }

    /// See `Model::select_elements`.
    pub(super) fn select_elements_impl(&self, expression: &Expression) -> Vec<ElementId> {
        self.eval_elements(expression).into_iter().collect()
    }

    /// See `Model::select_relationships`.
    pub(super) fn select_relationships_impl(&self, expression: &Expression) -> Vec<RelationshipId> {
        self.eval_relationships(expression).into_iter().collect()
    }
}

/// The items in every set.
fn intersect<T: Ord + Clone>(mut sets: impl Iterator<Item = BTreeSet<T>>) -> BTreeSet<T> {
    let Some(first) = sets.next() else {
        return BTreeSet::new();
    };
    sets.fold(first, |acc, s| acc.intersection(&s).cloned().collect())
}

/// True when `operand` is a relationship-expression operand: a
/// `relationship` form, or an arrow with something on each side (`*->*`,
/// `a -> b`); `->x`, `x->` and `->x->` are element couplings.
fn is_relationship_operand(operand: &str) -> bool {
    let lower = operand.to_ascii_lowercase();
    if lower.starts_with("relationship") {
        return true;
    }
    let coupling = lower.strip_prefix("element==").unwrap_or(&lower).trim();
    coupling
        .split_once("->")
        .is_some_and(|(s, d)| !s.trim().is_empty() && !d.trim().is_empty())
}

/// The element kind of an `element.type` name, ignoring case; `None` for
/// a type phase 2 does not build.
fn type_kind(name: &str) -> Option<ElementKind> {
    [
        ("person", ElementKind::Person),
        ("softwaresystem", ElementKind::SoftwareSystem),
        ("container", ElementKind::Container),
        ("component", ElementKind::Component),
        ("custom", ElementKind::Custom),
    ]
    .into_iter()
    .find(|(n, _)| n.eq_ignore_ascii_case(name))
    .map(|(_, k)| k)
}

/// `name` and `value` of `properties[name]==value`.
fn property(rest: &str) -> Option<(String, String)> {
    let rest = rest.strip_prefix("properties[")?;
    let (name, value) = rest.split_once("]==")?;
    Some((name.to_owned(), value.to_owned()))
}

/// `a,b` as trimmed, non-empty tags.
fn tag_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Builder<'_> {
    /// `text`, the written expression, as an `Expression` selecting
    /// `items`. `span` is the literal it came from; `context` the block it
    /// is read in.
    fn expression(
        &self,
        text: &str,
        items: Items,
        span: Span,
        context: Option<ElementId>,
    ) -> Result<Expression, ModelError> {
        let parts: Vec<&str> = if text.contains("&&") {
            text.split("&&").collect()
        } else {
            text.split("||").collect()
        };
        let mut atoms = Vec::new();
        for part in &parts {
            atoms.push(self.atom(part.trim(), items, span, context)?);
        }
        Ok(match atoms.len() {
            1 => atoms.remove(0),
            _ if text.contains("&&") => Expression::And(atoms),
            _ => Expression::Or(atoms),
        })
    }

    /// An expression in a view's `include` or `exclude` (P2-20): a
    /// relationship expression when its first operand is one (a
    /// `relationship...` form, or `a -> b` with something on both sides
    /// of the arrow), else an element expression; every operand is read
    /// as that kind.
    // DECISION: Structurizr reads each operand on its own and intersects
    // or unites what they select; an element operand next to a
    // relationship operand selects nothing useful there, so c4rizer reads
    // every operand as the first one's kind, and a mismatched one is an
    // error naming it.
    pub(super) fn view_expression(&self, text: &str, span: Span) -> Result<Expression, ModelError> {
        let first = if text.contains("&&") {
            text.split("&&").next()
        } else {
            text.split("||").next()
        }
        .unwrap_or(text)
        .trim();
        let items = if is_relationship_operand(first) {
            Items::Relationships
        } else {
            Items::Elements
        };
        self.expression(text, items, span, None)
    }

    /// The error for an operand that is no expression form.
    fn bad_operand(&self, text: &str, items: Items, span: Span) -> ModelError {
        self.error(
            ModelErrorKind::InvalidValue {
                construct: match items {
                    Items::Elements => "!elements".into(),
                    Items::Relationships => "!relationships".into(),
                },
                value: text.to_owned(),
            },
            span,
        )
    }

    /// The element `t` names, or `UndeclaredId`.
    fn operand_id(&self, t: &str, span: Span, context: Option<ElementId>) -> Result<ElementId, ModelError> {
        self.resolve(t, context)
            .ok_or_else(|| self.error(ModelErrorKind::UndeclaredId { id: t.to_owned() }, span))
    }

    /// One operand of an expression.
    fn atom(
        &self,
        text: &str,
        items: Items,
        span: Span,
        context: Option<ElementId>,
    ) -> Result<Expression, ModelError> {
        match items {
            Items::Elements => self.element_atom(text, span, context),
            Items::Relationships => self.relationship_atom(text, span, context),
        }
    }

    /// An element-expression operand (/dsl/expressions).
    fn element_atom(
        &self,
        text: &str,
        span: Span,
        context: Option<ElementId>,
    ) -> Result<Expression, ModelError> {
        let lower = text.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("element.") {
            let raw = &text["element.".len()..];
            let value = |op: &str| raw[op.len()..].to_owned();
            return Ok(if rest.starts_with("type==") {
                let name = value("type==");
                Expression::ElementType {
                    kind: type_kind(&name),
                    name,
                }
            } else if rest.starts_with("tag==") || rest.starts_with("tag!=") {
                Expression::ElementTag {
                    tags: tag_list(&value("tag==")),
                    negated: rest.starts_with("tag!="),
                }
            } else if rest.starts_with("technology==") || rest.starts_with("technology!=") {
                Expression::ElementTechnology {
                    technology: value("technology=="),
                    negated: rest.starts_with("technology!="),
                }
            } else if rest.starts_with("parent==") {
                Expression::ElementParent(self.operand_id(&value("parent=="), span, context)?)
            } else if rest.starts_with("group==") {
                Expression::ElementGroup(value("group=="))
            } else if let Some((name, value)) = property(raw) {
                Expression::ElementProperty { name, value }
            } else {
                return Err(self.bad_operand(text, Items::Elements, span));
            });
        }
        let coupling = lower
            .strip_prefix("element==")
            .map_or(text, |_| &text["element==".len()..]);
        let target = |t: &str| -> Result<Box<Expression>, ModelError> {
            Ok(Box::new(self.element_or_group(t, span, context)?))
        };
        if let Some(inner) = coupling.strip_prefix("->") {
            return Ok(match inner.strip_suffix("->") {
                Some(x) => Expression::AfferentEfferent(target(x)?),
                None => Expression::Afferent(target(inner)?),
            });
        }
        if let Some(x) = coupling.strip_suffix("->") {
            return Ok(Expression::Efferent(target(x)?));
        }
        self.element_or_group(coupling, span, context)
    }

    /// A relationship-expression operand (/dsl/expressions).
    fn relationship_atom(
        &self,
        text: &str,
        span: Span,
        context: Option<ElementId>,
    ) -> Result<Expression, ModelError> {
        let end = |t: &str| -> Result<Option<ElementId>, ModelError> {
            if t == "*" {
                Ok(None)
            } else {
                self.operand_id(t, span, context).map(Some)
            }
        };
        // P2-333: a side of `a -> b` may be an element expression; it
        // stands for the elements it selects when the line is read.
        let side = |t: &str| -> Result<Vec<Option<ElementId>>, ModelError> {
            let lower = t.to_ascii_lowercase();
            let expression = t != "*"
                && self.resolve(t, context).is_none()
                && (lower.starts_with("element") || t.contains("==") || t.contains("!="));
            if expression {
                let e = self.element_atom(t, span, context)?;
                Ok(self.model.select_elements(&e).into_iter().map(Some).collect())
            } else {
                Ok(vec![end(t)?])
            }
        };
        let lower = text.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("relationship.") {
            let raw = &text["relationship.".len()..];
            let value = |op: &str| raw[op.len()..].to_owned();
            return Ok(if rest.starts_with("tag==") || rest.starts_with("tag!=") {
                Expression::RelationshipTag {
                    tags: tag_list(&value("tag==")),
                    negated: rest.starts_with("tag!="),
                }
            } else if rest.starts_with("source==") {
                Expression::Relationships {
                    source: end(&value("source=="))?,
                    destination: None,
                }
            } else if rest.starts_with("destination==") {
                Expression::Relationships {
                    source: None,
                    destination: end(&value("destination=="))?,
                }
            } else if let Some((name, value)) = property(raw) {
                Expression::RelationshipProperty { name, value }
            } else {
                return Err(self.bad_operand(text, Items::Relationships, span));
            });
        }
        let pattern = lower
            .strip_prefix("relationship==")
            .map_or(text, |_| &text["relationship==".len()..]);
        if pattern == "*" {
            return Ok(Expression::Relationships {
                source: None,
                destination: None,
            });
        }
        let Some((s, d)) = pattern.split_once("->") else {
            return Err(self.bad_operand(text, Items::Relationships, span));
        };
        let (sources, destinations) = (side(s.trim())?, side(d.trim())?);
        let mut pairs: Vec<Expression> = sources
            .iter()
            .flat_map(|&source| {
                destinations.iter().map(move |&destination| Expression::Relationships {
                    source,
                    destination,
                })
            })
            .collect();
        // DECISION: an expression side that selects no element gives an
        // empty `Or`, which matches nothing.
        Ok(if pairs.len() == 1 {
            pairs.remove(0)
        } else {
            Expression::Or(pairs)
        })
    }

    /// An element id, or a group id (its members).
    fn element_or_group(
        &self,
        text: &str,
        span: Span,
        context: Option<ElementId>,
    ) -> Result<Expression, ModelError> {
        if let Some(id) = self.resolve(text, context) {
            return Ok(Expression::Element(id));
        }
        let group = self.model.groups.iter().find(|g| {
            g.key
                .as_ref()
                .is_some_and(|k| k.value.eq_ignore_ascii_case(text))
        });
        match group {
            Some(g) => Ok(Expression::Group(g.id)),
            None => Err(self.error(ModelErrorKind::UndeclaredId { id: text.to_owned() }, span)),
        }
    }

    /// Resolves the relationship ends that can be resolved now, so an
    /// expression sees the relationships written above it.
    fn resolve_ready(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for p in pending {
            match self.resolve(&p.key, p.context) {
                Some(id) => {
                    let r = &mut self.model.relationships[p.relationship];
                    if p.source {
                        r.source.value = id;
                    } else {
                        r.destination.value = id;
                    }
                }
                None => self.pending.push(p),
            }
        }
    }

    /// The expression of `!elements <expr>` / `!relationships <expr>`.
    fn directive_expression(
        &mut self,
        head: &Head<'_>,
        items: Items,
        context: Option<ElementId>,
    ) -> Result<Expression, ModelError> {
        if head.len() < 2 {
            return Err(self.missing(head, "expression"));
        }
        if head.len() > 2 {
            return Err(self.invalid(head, head.text(0), 2));
        }
        if !head.is_string(1) {
            return Err(self.invalid(head, head.text(0), 1));
        }
        self.resolve_ready();
        let literal = head.literal(1);
        self.expression(&literal.value, items, literal.span, context)
    }

    /// `!elements <expression> { ... }` (P2-166): the block's `->`, `tag`,
    /// `tags`, `url`, `properties` and `perspectives` apply to every
    /// element the expression selects, in id order.
    // DECISION: Structurizr for Java refuses an expression that selects
    // nothing ("No elements found for expression"): `InvalidValue`.
    pub(super) fn elements_directive(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        context: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let expression = self.directive_expression(head, Items::Elements, context)?;
        let selected = self.model.select_elements(&expression);
        if selected.is_empty() {
            return Err(self.invalid(head, "!elements", 1));
        }
        let block = self.block(statement, head)?;
        for id in selected {
            for inner in block {
                let line = self.head(inner)?;
                if self.common(inner, &line)? {
                    continue;
                }
                let allowed = line.arrow().is_some()
                    || ["tag", "tags", "url", "properties", "perspectives"]
                        .iter()
                        .any(|k| line.is_keyword(0, k));
                if !allowed {
                    return Err(self.reject(&line, 0));
                }
                if line.arrow().is_some() {
                    self.relationship(inner, &line, Some(id), None)?;
                } else {
                    self.element_line(inner, &line, id)?;
                }
            }
            self.retag_element(id);
        }
        Ok(())
    }

    /// `!relationships <expression> { ... }` (P2-168): the block's `tag`,
    /// `tags`, `url`, `properties` and `perspectives` apply to every
    /// relationship the expression selects.
    pub(super) fn relationships_directive(
        &mut self,
        statement: &Statement,
        head: &Head<'_>,
        context: Option<ElementId>,
    ) -> Result<(), ModelError> {
        let expression = self.directive_expression(head, Items::Relationships, context)?;
        let selected = self.model.select_relationships(&expression);
        if selected.is_empty() {
            return Err(self.invalid(head, "!relationships", 1));
        }
        let block = self.block(statement, head)?;
        for r in selected {
            let index = r.0 as usize;
            for inner in block {
                let line = self.head(inner)?;
                if self.common(inner, &line)? {
                    continue;
                }
                self.relationship_line(inner, &line, index)?;
            }
            self.retag_relationship(index);
        }
        Ok(())
    }
}
