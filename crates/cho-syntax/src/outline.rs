//! Declarations in a file, with qualified names and byte ranges.
//!
//! This is the structural layer under the outline view and under anchor
//! resolution for investigations. It is purely syntactic: it never guesses
//! what a name *refers to*, only where things are declared.

use crate::Language;
use std::ops::Range;
use tree_sitter::{Node, Parser, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Impl,
    Module,
    Type,
    Constant,
    /// A field that isn't a constant (Java).
    Field,
    Macro,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Impl => "impl",
            Self::Module => "module",
            Self::Type => "type",
            Self::Constant => "constant",
            Self::Field => "field",
            Self::Macro => "macro",
        }
    }

    fn is_callable(self) -> bool {
        matches!(self, Self::Function | Self::Method)
    }
}

/// One declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// Container names from outermost to innermost, e.g. `["Queue"]` for `Queue::push`.
    pub scope: Vec<String>,
    pub kind: SymbolKind,
    /// The whole declaration, including its body.
    pub range: Range<usize>,
    /// Just the name token.
    pub name_range: Range<usize>,
}

impl Symbol {
    /// The name as the language writes a qualified path (`Queue::push`, `Queue.push`).
    pub fn qualified_name(&self, language: Language) -> String {
        let sep = if language == Language::Rust {
            "::"
        } else {
            "."
        };
        let mut parts = self.scope.clone();
        parts.push(self.name.clone());
        parts.join(sep)
    }
}

/// Parses `source` and lists its declarations in source order. Empty for the highlight-only
/// data formats.
pub fn outline(language: Language, source: &str) -> Vec<Symbol> {
    if !language.has_structure() {
        return Vec::new();
    }
    match parse(language, source) {
        Some(tree) => outline_tree(language, &tree, source),
        None => Vec::new(),
    }
}

pub(crate) fn parse(language: Language, source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language.grammar()).ok()?;
    parser.parse(source, None)
}

fn outline_tree(language: Language, tree: &Tree, source: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    walk(language, tree.root_node(), source, &mut out);
    out
}

/// What a node declares: its kind, name node, and whether it opens a scope
/// for the declarations inside it.
struct Decl<'t> {
    kind: SymbolKind,
    name: Node<'t>,
    /// Name pushed onto the scope for children; usually the declared name.
    scope_name: Option<String>,
    /// `false` for containers like Rust `impl`, which aren't listed themselves.
    listed: bool,
    /// The declaration's extent when it isn't the node's own (a Java field's only declarator
    /// covers the whole `private int x = 1;`).
    range: Option<Range<usize>>,
}

fn text<'s>(node: Node, source: &'s str) -> &'s str {
    &source[node.byte_range()]
}

/// The last identifier-like segment of a type expression (`foo::Bar<T>` → `Bar`).
fn type_name(node: Node, source: &str) -> String {
    let raw = text(node, source);
    let base = raw.split(['<', '(', '[', ' ']).next().unwrap_or(raw);
    base.rsplit([':', '.'])
        .next()
        .unwrap_or(base)
        .trim()
        .to_string()
}

fn declaration<'t>(
    language: Language,
    node: Node<'t>,
    source: &str,
    in_container: bool,
) -> Option<Decl<'t>> {
    use SymbolKind::*;
    let named = |kind: SymbolKind| {
        let name = node.child_by_field_name("name")?;
        Some(Decl {
            kind,
            name,
            scope_name: Some(text(name, source).to_string()),
            listed: true,
            range: None,
        })
    };
    let callable = || named(if in_container { Method } else { Function });
    match language {
        Language::Rust => match node.kind() {
            "function_item" | "function_signature_item" => callable(),
            "struct_item" | "union_item" => named(Struct),
            "enum_item" => named(Enum),
            "trait_item" => named(Interface),
            "mod_item" => named(Module),
            "type_item" => named(Type),
            "const_item" | "static_item" => named(Constant),
            "macro_definition" => named(Macro),
            "impl_item" => {
                let ty = node.child_by_field_name("type")?;
                Some(Decl {
                    kind: Impl,
                    name: ty,
                    scope_name: Some(type_name(ty, source)),
                    listed: false,
                    range: None,
                })
            }
            _ => None,
        },
        Language::Python => match node.kind() {
            "function_definition" => callable(),
            "class_definition" => named(Class),
            _ => None,
        },
        Language::Java => java_declaration(node, source),
        Language::TypeScript | Language::Tsx | Language::JavaScript => match node.kind() {
            "function_declaration" | "generator_function_declaration" | "function_signature" => {
                callable()
            }
            "method_definition" | "method_signature" | "abstract_method_signature" => named(Method),
            "class_declaration" | "abstract_class_declaration" | "class" => named(Class),
            "interface_declaration" => named(Interface),
            "enum_declaration" => named(Enum),
            "type_alias_declaration" => named(Type),
            "internal_module" | "module" => named(Module),
            // `const handler = () => {}` and `const f = function () {}`
            "variable_declarator" => {
                let value = node.child_by_field_name("value")?;
                matches!(
                    value.kind(),
                    "arrow_function" | "function_expression" | "function"
                )
                .then(callable)
                .flatten()
            }
            _ => None,
        },
        Language::Swift => match node.kind() {
            "function_declaration" | "protocol_function_declaration" => callable(),
            "class_declaration" => {
                // Covers class, struct, enum, actor and extension; the keyword says which.
                let keyword = node
                    .child_by_field_name("declaration_kind")
                    .map(|k| text(k, source));
                let kind = match keyword {
                    Some("struct") => Struct,
                    Some("enum") => Enum,
                    Some("extension") => Impl,
                    _ => Class,
                };
                let name = node.child_by_field_name("name")?;
                let scope = type_name(name, source);
                Some(Decl {
                    kind,
                    name,
                    scope_name: Some(scope),
                    listed: kind != Impl,
                    range: None,
                })
            }
            "protocol_declaration" => named(Interface),
            "typealias_declaration" => named(Type),
            "init_declaration" => {
                let name = node.child(0)?;
                Some(Decl {
                    kind: Method,
                    name,
                    scope_name: None,
                    listed: true,
                    range: None,
                })
            }
            _ => None,
        },
        // Highlight-only: `outline` returns before parsing them.
        Language::Json | Language::Toml | Language::Xml | Language::Markdown => None,
    }
}

/// Java: types, members, enum constants and fields. Package and module names aren't listed
/// (they're dotted paths, not declarations a query can name). An anonymous class body is an
/// unlisted container named `new T`, so its methods read `Outer.new Comparator.compare`
/// rather than claiming to belong to `Outer`; lambdas declare nothing.
fn java_declaration<'t>(node: Node<'t>, source: &str) -> Option<Decl<'t>> {
    use SymbolKind::*;
    let named = |kind: SymbolKind| {
        let name = node.child_by_field_name("name")?;
        (name.kind() == "identifier").then(|| Decl {
            kind,
            name,
            scope_name: Some(text(name, source).to_string()),
            listed: true,
            range: None,
        })
    };
    match node.kind() {
        "class_declaration" | "record_declaration" => named(Class),
        "interface_declaration" | "annotation_type_declaration" => named(Interface),
        "enum_declaration" => named(Enum),
        "method_declaration"
        | "constructor_declaration"
        | "compact_constructor_declaration"
        | "annotation_type_element_declaration" => named(Method),
        // An enum constant with a body scopes the methods in it: `Op.PLUS.apply`.
        "enum_constant" => named(Constant),
        // `new Comparator<T>() { … }`: the body scopes what's declared in it.
        "class_body" => {
            let creation = node
                .parent()
                .filter(|p| p.kind() == "object_creation_expression")?;
            let ty = creation.child_by_field_name("type")?;
            let ty_name = type_name(ty, source);
            (!ty_name.is_empty()).then(|| Decl {
                kind: Class,
                name: ty,
                scope_name: Some(format!("new {ty_name}")),
                listed: false,
                range: None,
            })
        }
        "variable_declarator" => {
            let field = node.parent()?;
            let constant = match field.kind() {
                // Interface fields are implicitly `static final`.
                "constant_declaration" => true,
                "field_declaration" => {
                    let modifiers = field
                        .children(&mut field.walk())
                        .find(|c| c.kind() == "modifiers");
                    modifiers.is_some_and(|m| {
                        let words: Vec<&str> =
                            m.children(&mut m.walk()).map(|c| c.kind()).collect();
                        words.contains(&"static") && words.contains(&"final")
                    })
                }
                _ => return None,
            };
            let name = node.child_by_field_name("name")?;
            let declarators = field
                .children_by_field_name("declarator", &mut field.walk())
                .count();
            Some(Decl {
                kind: if constant { Constant } else { Field },
                name,
                // A field's initialiser (a lambda, an anonymous class) is scoped by the type.
                scope_name: None,
                listed: true,
                range: (declarators == 1).then(|| field.byte_range()),
            })
        }
        _ => None,
    }
}

/// Declarations inside more named scopes than this are not listed: each symbol carries its
/// whole scope path, so a hostile file nesting 20,000 modules would cost minutes and
/// gigabytes. Real code nests a handful deep.
pub const MAX_SCOPE_DEPTH: usize = 64;

/// Visits every node in source order with a cursor, not recursion: a repository file can
/// nest deeper than any thread's stack.
fn walk(language: Language, root: Node, source: &str, out: &mut Vec<Symbol>) {
    let mut scope = Vec::new();
    // Per open node: did entering it push a scope name?
    let mut pushed: Vec<bool> = Vec::new();
    let mut cursor = root.walk();
    loop {
        pushed.push(enter(language, cursor.node(), source, &mut scope, out));
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if pushed.pop() == Some(true) {
                scope.pop();
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// Lists `node` if it is a declaration; `true` when it pushed a scope name for its children.
fn enter(
    language: Language,
    node: Node,
    source: &str,
    scope: &mut Vec<String>,
    out: &mut Vec<Symbol>,
) -> bool {
    if scope.len() >= MAX_SCOPE_DEPTH {
        return false;
    }
    let in_container = !scope.is_empty();
    // Error recovery can produce zero-width (missing) name nodes; a declaration
    // with no name is not a declaration Chorong can point at, so skip it entirely.
    let decl = declaration(language, node, source, in_container)
        .filter(|d| !text(d.name, source).trim().is_empty());
    let mut pushed = false;
    if let Some(decl) = &decl {
        if decl.listed {
            let name = if language == Language::Swift && node.kind() == "init_declaration" {
                "init".to_string()
            } else {
                text(decl.name, source).to_string()
            };
            out.push(Symbol {
                name,
                scope: scope.clone(),
                kind: decl.kind,
                range: decl.range.clone().unwrap_or_else(|| node.byte_range()),
                name_range: decl.name.byte_range(),
            });
        }
        // Functions don't qualify what's inside them: a nested helper is
        // still listed, but under its enclosing type, not the function.
        if !decl.kind.is_callable()
            && let Some(s) = &decl.scope_name
            && !s.is_empty()
        {
            scope.push(s.clone());
            pushed = true;
        }
    }
    pushed
}

/// How an anchor's symbol matched the outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolMatch {
    /// Exactly one declaration matches.
    Unique(Symbol),
    /// Several declarations match (overloads, `#[cfg]` alternatives, …).
    Ambiguous(Vec<Symbol>),
    NotFound,
}

/// Finds declarations named by `query`, which may be qualified with `::` or
/// `.` (`Queue::push`, `Queue.push`, `push`). A qualified query must match the
/// trailing part of the declaration's scope; an unqualified one matches any scope. A
/// declaration named exactly `query` (a Swift operator such as `...` or `..<`) matches
/// first, since its name can't be split.
pub fn find_symbol(language: Language, source: &str, query: &str) -> SymbolMatch {
    let symbols = outline(language, source);
    let exact: Vec<Symbol> = symbols
        .iter()
        .filter(|s| s.name == query.trim())
        .cloned()
        .collect();
    let mut hits = if !exact.is_empty() && query.trim().contains(['.', ':']) {
        exact
    } else {
        qualified(symbols, query)
    };
    match hits.len() {
        0 => SymbolMatch::NotFound,
        1 => SymbolMatch::Unique(hits.remove(0)),
        _ => SymbolMatch::Ambiguous(hits),
    }
}

/// The declarations `query` names as `Scope::name` or `Scope.name`.
fn qualified(symbols: Vec<Symbol>, query: &str) -> Vec<Symbol> {
    let wanted: Vec<&str> = query
        .split("::")
        .flat_map(|p| p.split('.'))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let Some((name, qualifiers)) = wanted.split_last() else {
        return vec![];
    };
    symbols
        .into_iter()
        .filter(|s| s.name == *name && s.scope.ends_with_strs(qualifiers))
        .collect()
}

trait EndsWithStrs {
    fn ends_with_strs(&self, tail: &[&str]) -> bool;
}

impl EndsWithStrs for Vec<String> {
    fn ends_with_strs(&self, tail: &[&str]) -> bool {
        tail.len() <= self.len()
            && self[self.len() - tail.len()..]
                .iter()
                .zip(tail)
                .all(|(a, b)| a == b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(language: Language, source: &str) -> Vec<(String, &'static str)> {
        outline(language, source)
            .iter()
            .map(|s| (s.qualified_name(language), s.kind.as_str()))
            .collect()
    }

    fn pairs(v: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
        v.iter().map(|(a, b)| (a.to_string(), *b)).collect()
    }

    #[test]
    fn rust() {
        let src = r#"
mod jobs {
    pub struct Queue;
    impl Queue { pub fn push(&self) { fn helper() {} } }
    impl<T> Drop for Wrapper<T> { fn drop(&mut self) {} }
    pub trait Sink { fn send(&self); }
}
fn main() {}
macro_rules! m { () => {} }
"#;
        assert_eq!(
            names(Language::Rust, src),
            pairs(&[
                ("jobs", "module"),
                ("jobs::Queue", "struct"),
                ("jobs::Queue::push", "method"),
                ("jobs::Queue::helper", "method"),
                ("jobs::Wrapper::drop", "method"),
                ("jobs::Sink", "interface"),
                ("jobs::Sink::send", "method"),
                ("main", "function"),
                ("m", "macro"),
            ])
        );
    }

    #[test]
    fn python() {
        let src = "class A:\n    def f(self):\n        pass\n\ndef g():\n    pass\n";
        assert_eq!(
            names(Language::Python, src),
            pairs(&[("A", "class"), ("A.f", "method"), ("g", "function")])
        );
    }

    #[test]
    fn typescript() {
        let src = "export class Svc { run(): void {} }\ninterface I { x(): void }\nconst h = () => 1;\nfunction top() {}\ntype T = string;\n";
        assert_eq!(
            names(Language::TypeScript, src),
            pairs(&[
                ("Svc", "class"),
                ("Svc.run", "method"),
                ("I", "interface"),
                ("I.x", "method"),
                ("h", "function"),
                ("top", "function"),
                ("T", "type"),
            ])
        );
    }

    #[test]
    fn javascript_and_tsx() {
        let js = "class A { m() {} }\nfunction f() {}\n";
        assert_eq!(
            names(Language::JavaScript, js),
            pairs(&[("A", "class"), ("A.m", "method"), ("f", "function")])
        );
        let tsx = "export function View() { return <div />; }\n";
        assert_eq!(names(Language::Tsx, tsx), pairs(&[("View", "function")]));
    }

    #[test]
    fn swift() {
        let src = "struct Point { func len() -> Int { 0 } }\nextension Point { init() {} }\nprotocol P { func p() }\nfunc free() {}\n";
        assert_eq!(
            names(Language::Swift, src),
            pairs(&[
                ("Point", "struct"),
                ("Point.len", "method"),
                ("Point.init", "method"),
                ("P", "interface"),
                ("P.p", "method"),
                ("free", "function"),
            ])
        );
    }

    /// Found by the large-input fuzz runs: the outline walk recursed once per tree level, so
    /// deeply nested source (here 100,000 braces) overflowed the stack and aborted Chorong.
    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let src = format!(
            "mod a {{ fn f() {}{} }}",
            "{".repeat(100_000),
            "}".repeat(100_000)
        );
        // A test thread's stack is 2 MB, as small as any thread Chorong outlines on.
        let names: Vec<String> = outline(Language::Rust, &src)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["a", "f"]);
        // Nesting past MAX_SCOPE_DEPTH stops listing instead of growing quadratically.
        let n = 20_000;
        let src = format!("{}{}", "mod a { ".repeat(n), "}".repeat(n));
        let started = std::time::Instant::now();
        let symbols = outline(Language::Rust, &src);
        assert_eq!(symbols.len(), MAX_SCOPE_DEPTH);
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    /// Found by the `outline` fuzz target: an operator named with dots was listed but
    /// could not be found, because the query was split at its dots.
    #[test]
    fn operators_with_dots_are_found() {
        let src = "protocol P {\n  func ....f()\n}\nfunc .+.(a: Int, b: Int) {}\n";
        for op in ["....", ".+."] {
            assert!(
                outline(Language::Swift, src).iter().any(|s| s.name == op),
                "{op}"
            );
            assert!(
                matches!(
                    find_symbol(Language::Swift, src, op),
                    SymbolMatch::Unique(_)
                ),
                "{op}"
            );
        }
    }

    #[test]
    fn java() {
        let src = r#"package com.example.jobs;

import java.util.*;

@interface Retry { int times() default 3; }

public class Queue<T extends Comparable<T>> implements Iterable<T> {
    public static final int MAX = 10;
    private final List<T> items = new ArrayList<>(), spare;
    private Runnable hook = () -> System.out.println("hi");
    private final Comparator<T> order = new Comparator<T>() {
        @Override public int compare(T a, T b) { return a.compareTo(b); }
    };

    public Queue() { this(MAX); }
    Queue(int cap) { spare = null; }

    @Retry public <R> R push(T item) {
        class Local { void help() {} }
        Runnable r = () -> { };
        return null;
    }

    static class Node { T value; }
    interface Visitor { int LIMIT = 1; void visit(Node n); }
    enum Op {
        PLUS { int apply(int a, int b) { return a + b; } },
        MINUS;
        int apply(int a, int b) { return 0; }
    }
}
"#;
        assert_eq!(
            names(Language::Java, src),
            pairs(&[
                ("Retry", "interface"),
                ("Retry.times", "method"),
                ("Queue", "class"),
                ("Queue.MAX", "constant"),
                ("Queue.items", "field"),
                ("Queue.spare", "field"),
                ("Queue.hook", "field"),
                ("Queue.order", "field"),
                ("Queue.new Comparator.compare", "method"),
                ("Queue.Queue", "method"),
                ("Queue.Queue", "method"),
                ("Queue.push", "method"),
                ("Queue.Local", "class"),
                ("Queue.Local.help", "method"),
                ("Queue.Node", "class"),
                ("Queue.Node.value", "field"),
                ("Queue.Visitor", "interface"),
                ("Queue.Visitor.LIMIT", "constant"),
                ("Queue.Visitor.visit", "method"),
                ("Queue.Op", "enum"),
                ("Queue.Op.PLUS", "constant"),
                ("Queue.Op.PLUS.apply", "method"),
                ("Queue.Op.MINUS", "constant"),
                ("Queue.Op.apply", "method"),
            ])
        );
        // A field's only declarator covers the whole declaration; one of several doesn't.
        let max = outline(Language::Java, src)
            .into_iter()
            .find(|s| s.name == "MAX")
            .unwrap();
        assert_eq!(&src[max.range], "public static final int MAX = 10;");
        let spare = outline(Language::Java, src)
            .into_iter()
            .find(|s| s.name == "spare")
            .unwrap();
        assert_eq!(&src[spare.range], "spare");
        assert!(matches!(
            find_symbol(Language::Java, src, "Op.PLUS.apply"),
            SymbolMatch::Unique(_)
        ));
        assert!(matches!(
            find_symbol(Language::Java, src, "Queue.new Comparator.compare"),
            SymbolMatch::Unique(_)
        ));
        assert!(
            matches!(find_symbol(Language::Java, src, "Queue.Queue"), SymbolMatch::Ambiguous(v) if v.len() == 2)
        );
    }

    #[test]
    fn find_symbol_qualified_and_ambiguous() {
        let src = "struct A; impl A { fn new() {} }\nstruct B; impl B { fn new() {} }\n";
        assert!(
            matches!(find_symbol(Language::Rust, src, "A::new"), SymbolMatch::Unique(s) if s.scope == ["A"])
        );
        assert!(matches!(
            find_symbol(Language::Rust, src, "A.new"),
            SymbolMatch::Unique(_)
        ));
        assert!(
            matches!(find_symbol(Language::Rust, src, "new"), SymbolMatch::Ambiguous(v) if v.len() == 2)
        );
        assert_eq!(
            find_symbol(Language::Rust, src, "C::new"),
            SymbolMatch::NotFound
        );
        assert_eq!(find_symbol(Language::Rust, src, ""), SymbolMatch::NotFound);
    }

    #[test]
    fn computed_names_with_separators_are_findable() {
        // Found by fuzzing: `[u.]` was listed but `find_symbol` split it at the dot.
        for src in ["{[u.]() {}}", "class A { [Symbol.iterator]() {} }"] {
            for sym in outline(Language::JavaScript, src) {
                assert!(
                    !matches!(
                        find_symbol(Language::JavaScript, src, &sym.name),
                        SymbolMatch::NotFound
                    ),
                    "{sym:?}"
                );
            }
        }
    }

    #[test]
    fn broken_code_never_yields_unnamed_symbols() {
        // Found by fuzzing: error recovery inserts zero-width name nodes.
        for (language, src) in [
            (Language::JavaScript, "class { m() {} }\nfunction (\n"),
            (Language::Rust, "fn (\nimpl for {}\nstruct {"),
            (Language::Python, "def (:\nclass :\n"),
            (Language::TypeScript, "interface { x(): void }\n"),
            (Language::Swift, "func () {}\nstruct {"),
            (
                Language::Java,
                "class { void () {} }\nenum { , }\nnew () {};\nint = 1;",
            ),
        ] {
            for sym in outline(language, src) {
                assert!(!sym.name.trim().is_empty(), "{language:?}: {sym:?}");
                assert!(
                    sym.scope.iter().all(|s| !s.trim().is_empty()),
                    "{language:?}: {sym:?}"
                );
            }
        }
    }
}
