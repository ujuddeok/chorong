//! What kind of reference an identifier token is, by where it sits in the syntax tree.
//! Purely syntactic: it says "this token is the callee of a call" or "this token is in a type
//! position", never what the token resolves to. The graph (`docs/public/graph.md`) pairs these kinds
//! with SCIP-resolved symbols.

use crate::Language;
use crate::analysis::{call_name, callee_token, is_identifier};
use crate::outline::parse;
use std::collections::HashMap;
use std::ops::Range;
use tree_sitter::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefKind {
    Calls,
    Constructs,
    Renders,
    TypeUses,
    Implements,
    ReExports,
    References,
}

impl RefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::Constructs => "constructs",
            Self::Renders => "renders",
            Self::TypeUses => "type-uses",
            Self::Implements => "implements",
            Self::ReExports => "re-exports",
            Self::References => "references",
        }
    }
}

/// An identifier token and the kind of reference its position implies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefToken {
    pub range: Range<usize>,
    pub kind: RefKind,
}

/// Classifies every identifier token of `source`, sorted by position. Tokens with no
/// more specific kind are `References`. `None` if no tree could be produced. Empty for the
/// highlight-only data formats.
pub fn classify_references(language: Language, source: &str) -> Option<Vec<RefToken>> {
    if !language.has_structure() {
        return Some(Vec::new());
    }
    let tree = parse(language, source)?;
    // Pass 1: callee, constructor and JSX name tokens, keyed by start byte.
    let mut marked: HashMap<usize, RefKind> = HashMap::new();
    let mut leaves: Vec<Node> = vec![];
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            if is_identifier(node.kind()) && node.start_byte() < node.end_byte() {
                leaves.push(node);
            }
            continue;
        }
        if let Some(name) = call_name(language, node) {
            let kind = if matches!(node.kind(), "new_expression" | "object_creation_expression") {
                RefKind::Constructs
            } else {
                RefKind::Calls
            };
            marked.insert(name.start_byte(), kind);
        }
        if let Some((name, kind)) = special_name(language, node) {
            marked.insert(name.start_byte(), kind);
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        stack.extend(children);
    }
    // Pass 2: everything else, by ancestors.
    let mut out: Vec<RefToken> = leaves
        .into_iter()
        .map(|leaf| {
            let kind = marked
                .get(&leaf.start_byte())
                .copied()
                .unwrap_or_else(|| by_ancestors(language, leaf));
            RefToken {
                range: leaf.byte_range(),
                kind,
            }
        })
        .collect();
    out.sort_by_key(|t| t.range.start);
    out.dedup_by_key(|t| t.range.start);
    Some(out)
}

/// Struct literals (Rust) and JSX elements (TypeScript, JavaScript).
fn special_name<'t>(language: Language, node: Node<'t>) -> Option<(Node<'t>, RefKind)> {
    match (language, node.kind()) {
        (Language::Rust, "struct_expression") => {
            let name = node.child_by_field_name("name")?;
            Some((last_type_name(name)?, RefKind::Constructs))
        }
        (
            Language::TypeScript | Language::Tsx | Language::JavaScript,
            "jsx_opening_element" | "jsx_self_closing_element",
        ) => {
            let name = node.child_by_field_name("name")?;
            let token = match name.kind() {
                "identifier" => name,
                "member_expression" | "nested_identifier" => {
                    name.child_by_field_name("property")
                        .or_else(|| name.child_by_field_name("name"))?
                }
                _ => return None,
            };
            // Lower-case names are DOM elements, not references to a declaration.
            Some((token, RefKind::Renders))
        }
        _ => None,
    }
}

fn last_type_name<'t>(node: Node<'t>) -> Option<Node<'t>> {
    match node.kind() {
        "type_identifier" | "identifier" => Some(node),
        "scoped_type_identifier" | "scoped_identifier" => {
            last_type_name(node.child_by_field_name("name")?)
        }
        "generic_type" => last_type_name(node.child_by_field_name("type")?),
        _ => callee_token(node),
    }
}

fn by_ancestors(language: Language, leaf: Node) -> RefKind {
    let mut child = leaf;
    let mut type_position = leaf.kind() == "type_identifier";
    while let Some(parent) = child.parent() {
        let kind = parent.kind();
        match language {
            Language::Rust => match kind {
                "use_declaration" => {
                    let mut c = parent.walk();
                    let public = parent
                        .children(&mut c)
                        .any(|n| n.kind() == "visibility_modifier");
                    return if public {
                        RefKind::ReExports
                    } else {
                        RefKind::References
                    };
                }
                "impl_item" => {
                    if parent.child_by_field_name("trait").map(|t| t.id()) == Some(child.id()) {
                        return RefKind::Implements;
                    }
                    break;
                }
                "trait_bounds"
                | "bounded_type"
                | "generic_type"
                | "type_arguments"
                | "scoped_type_identifier"
                | "reference_type"
                | "array_type"
                | "tuple_type"
                | "function_type"
                | "abstract_type"
                | "dynamic_type" => type_position = true,
                "function_item" | "block" | "closure_expression" | "let_declaration"
                | "source_file" | "declaration_list" => break,
                _ => {}
            },
            Language::TypeScript | Language::Tsx | Language::JavaScript => match kind {
                "extends_clause"
                | "implements_clause"
                | "extends_type_clause"
                | "class_heritage" => return RefKind::Implements,
                "export_statement" => {
                    return if parent.child_by_field_name("source").is_some() {
                        RefKind::ReExports
                    } else if type_position {
                        RefKind::TypeUses
                    } else {
                        RefKind::References
                    };
                }
                "import_statement" => {
                    let mut c = parent.walk();
                    let type_only = parent.children(&mut c).any(|n| n.kind() == "type");
                    return if type_only || type_position {
                        RefKind::TypeUses
                    } else {
                        RefKind::References
                    };
                }
                "type_annotation"
                | "type_arguments"
                | "type_parameters"
                | "generic_type"
                | "union_type"
                | "intersection_type"
                | "array_type"
                | "type_alias_declaration"
                | "nested_type_identifier"
                | "predefined_type"
                | "interface_declaration" => type_position = true,
                "statement_block" | "program" | "class_body" | "arguments" => break,
                _ => {}
            },
            Language::Java => match kind {
                "superclass" | "super_interfaces" | "extends_interfaces" => {
                    return RefKind::Implements;
                }
                "import_declaration" | "package_declaration" => return RefKind::References,
                "type_arguments"
                | "type_parameters"
                | "generic_type"
                | "array_type"
                | "scoped_type_identifier"
                | "type_bound"
                | "throws" => type_position = true,
                "block" | "class_body" | "program" | "argument_list" => break,
                _ => {}
            },
            _ => break,
        }
        child = parent;
    }
    if type_position {
        RefKind::TypeUses
    } else {
        RefKind::References
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(language: Language, src: &str) -> Vec<(String, RefKind)> {
        classify_references(language, src)
            .unwrap()
            .into_iter()
            .map(|t| (src[t.range].to_string(), t.kind))
            .collect()
    }

    fn kind_of(language: Language, src: &str, name: &str) -> Vec<RefKind> {
        kinds(language, src)
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, k)| k)
            .collect()
    }

    #[test]
    fn rust_kinds() {
        let src = "pub use a::Re;\nuse b::Imp;\nimpl Tr for Ty { fn f(&self, x: Arg) { helper(); let s = Lit { a: 1 }; Ty::new(); } }";
        assert_eq!(kind_of(Language::Rust, src, "Re"), [RefKind::ReExports]);
        assert_eq!(kind_of(Language::Rust, src, "Imp"), [RefKind::References]);
        assert_eq!(kind_of(Language::Rust, src, "Tr"), [RefKind::Implements]);
        assert_eq!(
            kind_of(Language::Rust, src, "Ty"),
            [RefKind::TypeUses, RefKind::References]
        );
        assert_eq!(kind_of(Language::Rust, src, "Arg"), [RefKind::TypeUses]);
        assert_eq!(kind_of(Language::Rust, src, "helper"), [RefKind::Calls]);
        assert_eq!(kind_of(Language::Rust, src, "Lit"), [RefKind::Constructs]);
        assert_eq!(kind_of(Language::Rust, src, "new"), [RefKind::Calls]);
    }

    #[test]
    fn typescript_kinds() {
        let src = "import type { T } from './t';\nexport { R } from './r';\nclass A extends B implements I { m(x: Arg): void { go(); new C(); } }\nconst e = <Comp />;";
        let ts = Language::Tsx;
        assert_eq!(kind_of(ts, src, "T"), [RefKind::TypeUses]);
        assert_eq!(kind_of(ts, src, "R"), [RefKind::ReExports]);
        assert_eq!(kind_of(ts, src, "B"), [RefKind::Implements]);
        assert_eq!(kind_of(ts, src, "I"), [RefKind::Implements]);
        assert_eq!(kind_of(ts, src, "Arg"), [RefKind::TypeUses]);
        assert_eq!(kind_of(ts, src, "go"), [RefKind::Calls]);
        assert_eq!(kind_of(ts, src, "C"), [RefKind::Constructs]);
        assert_eq!(kind_of(ts, src, "Comp"), [RefKind::Renders]);
    }

    #[test]
    fn java_kinds() {
        let src = "import java.util.Imp;\nclass A extends B implements I, J { Arg m(Par p) throws Ex { go(); return new C<D>(); } }\ninterface K extends L {}";
        let j = Language::Java;
        assert_eq!(kind_of(j, src, "Imp"), [RefKind::References]);
        assert_eq!(kind_of(j, src, "B"), [RefKind::Implements]);
        assert_eq!(kind_of(j, src, "I"), [RefKind::Implements]);
        assert_eq!(kind_of(j, src, "J"), [RefKind::Implements]);
        assert_eq!(kind_of(j, src, "L"), [RefKind::Implements]);
        assert_eq!(kind_of(j, src, "Arg"), [RefKind::TypeUses]);
        assert_eq!(kind_of(j, src, "Par"), [RefKind::TypeUses]);
        assert_eq!(kind_of(j, src, "Ex"), [RefKind::TypeUses]);
        assert_eq!(kind_of(j, src, "go"), [RefKind::Calls]);
        assert_eq!(kind_of(j, src, "C"), [RefKind::Constructs]);
        assert_eq!(kind_of(j, src, "D"), [RefKind::TypeUses]);
    }

    #[test]
    fn python_defaults() {
        let src = "def f():\n    g()\n    return h\n";
        assert_eq!(kind_of(Language::Python, src, "g"), [RefKind::Calls]);
        assert_eq!(kind_of(Language::Python, src, "h"), [RefKind::References]);
    }
}
