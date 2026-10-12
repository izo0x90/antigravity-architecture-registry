//! What a call's target is made of, read from the syntax tree: a name, a
//! path of names (`a::b::f`), a member of a value (`x.f`), or what calling
//! something gives (`make().f`). Each pack says which syntax kinds are which
//! (`[[expressions]]`); the resolver works on these instead of taking the
//! text apart.

use serde::{Deserialize, Serialize};

use crate::extract::AstNode;
use crate::pack::{ExpressionShape, LanguageSpec, Pack};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    /// `helper`, `self`, `Store`.
    Name(String),
    /// Names joined by a path separator: `a::b::f` (parts, separator).
    Path(Vec<String>, String),
    /// A member of a value: `x.f` (the value, the member's name, the separator).
    Member(Box<Expr>, String, String),
    /// What calling it gives: `make()`.
    Call(Box<Expr>),
    /// Anything else (a literal, a closure, an index), as written.
    Other(String),
}

impl Expr {
    /// Reads `node` by the pack's expression shapes.
    pub(crate) fn read(pack: &Pack, node: &AstNode<'_>) -> Expr {
        let kind = node.kind();
        let Some(spec) = pack.spec.expressions.iter().find(|e| e.kinds.iter().any(|k| *k == kind)) else {
            return Expr::Other(node.text().trim().to_owned());
        };
        let inner = || match &spec.of {
            Some(field) => node.field(field),
            None => node.named_children().next(),
        };
        let name = || spec.name.as_ref().and_then(|f| node.field(f)).map(|n| n.text().trim().to_owned());
        let sep = || spec.separator.clone().unwrap_or_default();
        match spec.shape {
            ExpressionShape::Name => Expr::Name(node.text().trim().to_owned()),
            ExpressionShape::Unwrap => inner().map_or_else(|| Expr::Other(node.text().trim().to_owned()), |n| Expr::read(pack, &n)),
            ExpressionShape::Call => inner().map_or_else(|| Expr::Other(node.text().trim().to_owned()), |n| Expr::Call(Box::new(Expr::read(pack, &n)))),
            ExpressionShape::Member => match (inner(), name()) {
                (Some(of), Some(name)) => Expr::Member(Box::new(Expr::read(pack, &of)), name, sep()),
                _ => Expr::Other(node.text().trim().to_owned()),
            },
            ExpressionShape::Path => match (inner().map(|q| Expr::read(pack, &q)), name()) {
                // `::f` (no qualifier): just the name.
                (None, Some(name)) => Expr::Name(name),
                (Some(Expr::Name(head)), Some(name)) => Expr::Path(vec![head, name], sep()),
                (Some(Expr::Path(mut parts, s)), Some(name)) if s == sep() => {
                    parts.push(name);
                    Expr::Path(parts, s)
                }
                _ => Expr::Other(node.text().trim().to_owned()),
            },
        }
    }

    /// The node holding the name `read` ends in (`f` in `a::b::f`, `save`
    /// in `x.save`): where the call's target is written, for matching it
    /// with a language server's answers by position. None for a call's result.
    pub(crate) fn name_node<'r>(pack: &Pack, node: &AstNode<'r>) -> Option<AstNode<'r>> {
        let kind = node.kind();
        let spec = pack.spec.expressions.iter().find(|e| e.kinds.iter().any(|k| *k == kind))?;
        let inner = || match &spec.of {
            Some(field) => node.field(field),
            None => node.named_children().next(),
        };
        match spec.shape {
            ExpressionShape::Name => Some(node.clone()),
            ExpressionShape::Unwrap => Self::name_node(pack, &inner()?),
            ExpressionShape::Path | ExpressionShape::Member => spec.name.as_ref().and_then(|f| node.field(f)),
            ExpressionShape::Call => None,
        }
    }

    /// A name or path written as text where there's no syntax to read (a
    /// route's handler captured from a string of code): split at the
    /// language's separators. Only names and separators are expected.
    pub(crate) fn from_written(text: &str, lang: &LanguageSpec) -> Expr {
        let words: Vec<String> = lang.words(text).into_iter().map(str::to_owned).collect();
        let sep = lang.call_separators.iter().find(|s| text.contains(s.as_str())).cloned().unwrap_or_default();
        match words.len() {
            0 => Expr::Other(text.to_owned()),
            1 => Expr::Name(words.into_iter().next().unwrap_or_default()),
            _ => Expr::Path(words, sep),
        }
    }

    /// As it reads (arguments left out: `make().f`).
    pub fn render(&self) -> String {
        match self {
            Expr::Name(n) | Expr::Other(n) => n.clone(),
            Expr::Path(parts, sep) => parts.join(sep),
            Expr::Member(of, name, sep) => format!("{}{sep}{name}", of.render()),
            Expr::Call(callee) => format!("{}()", callee.render()),
        }
    }

    /// The name it ends in (`a::b::f`, `x.f` -> `f`).
    pub fn last_name(&self) -> Option<&str> {
        match self {
            Expr::Name(n) => Some(n),
            Expr::Path(parts, _) => parts.last().map(String::as_str),
            Expr::Member(_, name, _) => Some(name),
            Expr::Call(_) | Expr::Other(_) => None,
        }
    }

    /// Every name in it, when it's only names (`a::b.c` -> a, b, c).
    pub fn names(&self) -> Option<Vec<&str>> {
        match self {
            Expr::Name(n) => Some(vec![n.as_str()]),
            Expr::Path(parts, _) => Some(parts.iter().map(String::as_str).collect()),
            Expr::Member(of, name, _) => {
                let mut out = of.names()?;
                out.push(name);
                Some(out)
            }
            Expr::Call(_) | Expr::Other(_) => None,
        }
    }

    /// What it's a member or path step of, the separator, and the last name:
    /// `a::b::f` -> (`a::b`, `::`, `f`); `x.f` -> (`x`, `.`, `f`).
    pub fn split(&self) -> Option<(Expr, &str, &str)> {
        match self {
            Expr::Member(of, name, sep) => Some(((**of).clone(), sep, name)),
            Expr::Path(parts, sep) if parts.len() >= 2 => {
                let head = &parts[..parts.len() - 1];
                let qualifier = if head.len() == 1 { Expr::Name(head[0].clone()) } else { Expr::Path(head.to_vec(), sep.clone()) };
                Some((qualifier, sep, &parts[parts.len() - 1]))
            }
            _ => None,
        }
    }

    /// With its first name replaced (an import alias by what it stands for).
    pub fn with_head(&self, head: &str) -> Expr {
        match self {
            Expr::Name(_) => Expr::Name(head.to_owned()),
            Expr::Path(parts, sep) => {
                let mut parts = parts.clone();
                parts[0] = head.to_owned();
                Expr::Path(parts, sep.clone())
            }
            Expr::Member(of, name, sep) => Expr::Member(Box::new(of.with_head(head)), name.clone(), sep.clone()),
            Expr::Call(callee) => Expr::Call(Box::new(callee.with_head(head))),
            Expr::Other(text) => Expr::Other(text.clone()),
        }
    }

    /// Its first name, if it starts with one.
    pub fn head(&self) -> Option<&str> {
        match self {
            Expr::Name(n) => Some(n),
            Expr::Path(parts, _) => parts.first().map(String::as_str),
            Expr::Member(of, _, _) => of.head(),
            Expr::Call(callee) => callee.head(),
            Expr::Other(_) => None,
        }
    }
}

impl std::fmt::Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}
