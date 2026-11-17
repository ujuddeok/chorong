//! Token-level facts from one Tree-sitter parse: identifier tokens, call-name tokens, foldable
//! regions, and whether the parse is complete. Purely syntactic: a call name is the token that names the
//! callee in a call expression, not a claim about what it resolves to.

use crate::Language;
use crate::outline::parse;
use std::ops::Range;
use tree_sitter::Node;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Identifier-like leaf tokens (not comments or strings), sorted, disjoint.
    pub identifiers: Vec<Range<usize>>,
    /// Callee-name tokens of call expressions (and macro invocations), sorted, disjoint.
    pub calls: Vec<Range<usize>>,
    /// Identifier tokens that are plain binding sites: a `let`/`const`/`static` name or a
    /// function parameter, where the syntax alone says "this introduces the name". Patterns
    /// with destructuring, assignments and anything else ambiguous are left out. Sorted.
    pub bindings: Vec<Range<usize>>,
    /// Foldable regions, as byte ranges from the line that opens them to the end of the line
    /// that closes them: every construct between a matching `{}`, `[]` or `()` pair, each
    /// indented Python block (from its header, `def`/`if`/`else` ..., to its last line), and
    /// each run of top-level imports. Sorted by start. Short ones are dropped by the viewer.
    pub folds: Vec<Range<usize>>,
    /// The tree contains an error or missing node: the parse is partial.
    pub has_error: bool,
}

/// The opening and closing tokens of a bracketed construct.
const BRACKETS: [(&str, &str); 3] = [("{", "}"), ("[", "]"), ("(", ")")];

/// Top-level statements that import, per language: runs of them fold together.
fn is_import(language: Language, kind: &str) -> bool {
    match language {
        Language::Rust => matches!(kind, "use_declaration" | "extern_crate_declaration"),
        Language::TypeScript | Language::Tsx | Language::JavaScript => kind == "import_statement",
        Language::Java => kind == "import_declaration",
        Language::Python => matches!(
            kind,
            "import_statement" | "import_from_statement" | "future_import_statement"
        ),
        Language::Swift => kind == "import_declaration",
        Language::Json | Language::Toml | Language::Xml | Language::Markdown => false,
    }
}

fn is_comment(kind: &str) -> bool {
    matches!(
        kind,
        "comment" | "line_comment" | "block_comment" | "multiline_comment"
    )
}

/// The fold `node` opens, if any (see [`Analysis::folds`]; import runs are found separately).
fn fold_of(language: Language, node: Node<'_>, children: &[Node<'_>]) -> Option<Range<usize>> {
    if language == Language::Python && node.kind() == "block" {
        // An indented body: from its header (`def f():`, `else:`) to its last line.
        let header = node.parent().map_or(node.start_byte(), |p| p.start_byte());
        return Some(header..node.end_byte());
    }
    let (first, last) = (children.first()?, children.last()?);
    BRACKETS
        .iter()
        .any(|(open, close)| first.kind() == *open && last.kind() == *close)
        .then(|| node.byte_range())
}

/// Runs of two or more imports among `nodes` (top-level statements), comments allowed between.
fn import_runs(language: Language, nodes: &[Node<'_>], out: &mut Vec<Range<usize>>) {
    let mut run: Option<(usize, usize, usize)> = None; // (start, end, count)
    for n in nodes {
        if is_import(language, n.kind()) {
            run = Some(match run {
                Some((start, _, count)) => (start, n.end_byte(), count + 1),
                None => (n.start_byte(), n.end_byte(), 1),
            });
        } else if !is_comment(n.kind())
            && let Some((start, end, count)) = run.take()
            && count >= 2
        {
            out.push(start..end);
        }
    }
    if let Some((start, end, count)) = run
        && count >= 2
    {
        out.push(start..end);
    }
}

/// Parses `source` and extracts [`Analysis`]. `None` if no tree could be produced. Empty for
/// the highlight-only formats, which have no identifiers, calls or folds, except JSON, whose
/// objects and arrays fold ([`json_folds`]).
pub fn analyze(language: Language, source: &str) -> Option<Analysis> {
    if language == Language::Json {
        return json_folds(source);
    }
    if !language.has_structure() {
        return Some(Analysis::default());
    }
    let tree = parse(language, source)?;
    let mut out = Analysis {
        has_error: tree.root_node().has_error(),
        ..Analysis::default()
    };
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            if is_identifier(node.kind()) && node.start_byte() < node.end_byte() {
                out.identifiers.push(node.byte_range());
            }
            continue;
        }
        if let Some(name) = call_name(language, node) {
            out.calls.push(name.byte_range());
        }
        binding_sites(language, node, &mut out.bindings);
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        if let Some(f) = fold_of(language, node, &children) {
            out.folds.push(f);
        }
        if node.parent().is_none() {
            import_runs(language, &children, &mut out.folds);
        }
        stack.extend(children.into_iter().rev());
    }
    out.folds
        .sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    out.folds.dedup();
    out.identifiers.sort_by_key(|r| r.start);
    out.calls.sort_by_key(|r| r.start);
    out.calls.dedup();
    out.bindings.sort_by_key(|r| r.start);
    out.bindings.dedup();
    Some(out)
}

/// JSON (and JSONC, whose comments the grammar reads): every object and array folds, from the
/// line its `{` or `[` is on to the line of its `}` or `]`. Nothing else is read: JSON stays
/// highlight-only otherwise (no identifiers, calls or outline). Strings and numbers have no
/// children, so the walk visits only the containers and their members.
fn json_folds(source: &str) -> Option<Analysis> {
    let tree = parse(Language::Json, source)?;
    let mut out = Analysis {
        has_error: tree.root_node().has_error(),
        ..Analysis::default()
    };
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "object" | "array") {
            out.folds.push(node.byte_range());
        }
        stack.extend(node.children(&mut cursor).filter(|c| c.child_count() > 0));
    }
    out.folds
        .sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    Some(out)
}

/// Pushes the unambiguous binding-name tokens that `node` introduces.
fn binding_sites(language: Language, node: Node<'_>, out: &mut Vec<Range<usize>>) {
    let plain = |n: Option<Node<'_>>, out: &mut Vec<Range<usize>>| {
        if let Some(n) = n {
            // `let mut x` wraps the name; destructuring does not count.
            let n = if n.kind() == "mut_pattern" {
                n.named_child(0).filter(|c| c.kind() == "identifier")
            } else {
                Some(n)
            };
            if let Some(n) = n.filter(|n| n.kind() == "identifier") {
                out.push(n.byte_range());
            }
        }
    };
    match (language, node.kind()) {
        (Language::Rust, "let_declaration" | "parameter") => {
            plain(node.child_by_field_name("pattern"), out)
        }
        (Language::Rust, "const_item" | "static_item") => {
            plain(node.child_by_field_name("name"), out)
        }
        (Language::TypeScript | Language::Tsx | Language::JavaScript, "variable_declarator") => {
            plain(node.child_by_field_name("name"), out)
        }
        (
            Language::TypeScript | Language::Tsx | Language::JavaScript,
            "required_parameter" | "optional_parameter",
        ) => plain(node.child_by_field_name("pattern"), out),
        (Language::JavaScript, "formal_parameters") | (Language::Python, "parameters") => {
            let mut cursor = node.walk();
            for c in node.named_children(&mut cursor) {
                plain(Some(c), out);
            }
        }
        (Language::Python, "default_parameter") => plain(node.child_by_field_name("name"), out),
        // `int x = 1;` (each declarator of a local or field), `(String s)`, `catch (E e)`,
        // `for (T t : ts)`, `try (R r = …)`, and lambda parameters `x -> …`, `(a, b) -> …`.
        (Language::Java, "variable_declarator") => plain(node.child_by_field_name("name"), out),
        (
            Language::Java,
            "formal_parameter" | "catch_formal_parameter" | "enhanced_for_statement" | "resource",
        ) => plain(node.child_by_field_name("name"), out),
        (Language::Java, "inferred_parameters") => {
            let mut cursor = node.walk();
            for c in node.named_children(&mut cursor) {
                plain(Some(c), out);
            }
        }
        (Language::Java, "lambda_expression") => {
            plain(
                node.child_by_field_name("parameters")
                    .filter(|p| p.kind() == "identifier"),
                out,
            );
        }
        _ => {}
    }
}

pub(crate) fn is_identifier(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
            | "simple_identifier"
    )
}

/// The token naming the callee of `node`, if `node` is a call.
pub(crate) fn call_name<'t>(language: Language, node: Node<'t>) -> Option<Node<'t>> {
    match (language, node.kind()) {
        (Language::Rust, "call_expression") => callee_token(node.child_by_field_name("function")?),
        (Language::Rust, "macro_invocation") => callee_token(node.child_by_field_name("macro")?),
        (Language::Python, "call") => callee_token(node.child_by_field_name("function")?),
        (
            Language::TypeScript | Language::Tsx | Language::JavaScript,
            "call_expression" | "new_expression",
        ) => callee_token(
            node.child_by_field_name("function")
                .or_else(|| node.child_by_field_name("constructor"))?,
        ),
        (Language::Swift, "call_expression") => callee_token(node.named_child(0)?),
        (Language::Java, "method_invocation") => node.child_by_field_name("name"),
        (Language::Java, "object_creation_expression") => {
            callee_token(node.child_by_field_name("type")?)
        }
        _ => None,
    }
}

/// The last name in a callee expression: `a::b::f` -> `f`, `x.f` -> `f`, `f::<T>` -> `f`.
pub(crate) fn callee_token<'t>(mut node: Node<'t>) -> Option<Node<'t>> {
    loop {
        node = match node.kind() {
            "identifier"
            | "simple_identifier"
            | "property_identifier"
            | "field_identifier"
            | "type_identifier" => return Some(node),
            // Java's scoped type has no `name` field: the name is its last child.
            "scoped_identifier" | "scoped_type_identifier" => {
                node.child_by_field_name("name").or_else(|| {
                    node.named_child(u32::try_from(node.named_child_count().checked_sub(1)?).ok()?)
                })?
            }
            "field_expression" => node.child_by_field_name("field")?,
            "member_expression" => node.child_by_field_name("property")?,
            "attribute" => node.child_by_field_name("attribute")?,
            "generic_function" => node.child_by_field_name("function")?,
            // Java `new ArrayList<T>()`.
            "generic_type" => node
                .child_by_field_name("type")
                .or_else(|| node.named_child(0))?,
            "parenthesized_expression" | "await_expression" => node.named_child(0)?,
            "navigation_expression" => node.child_by_field_name("suffix")?,
            "navigation_suffix" => node.child_by_field_name("suffix")?,
            _ => return None,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fold regions as zero-based (first, last) lines, keeping those over three lines
    /// (the viewer's minimum).
    fn fold_lines(language: Language, src: &str) -> Vec<(usize, usize)> {
        let line = |b: usize| src[..b].matches('\n').count();
        let mut v: Vec<_> = analyze(language, src)
            .unwrap()
            .folds
            .iter()
            .map(|r| (line(r.start), line(r.end.saturating_sub(1))))
            .filter(|(a, b)| b - a + 1 > 3)
            .collect();
        v.dedup();
        v
    }

    const RUST: &str = "use a::b;\nuse c::d;\nuse e::f;\nuse g::h;\n\nimpl Q {\n    fn run(&self) {\n        if x {\n            one();\n            two();\n        } else {\n            three();\n            four();\n        }\n        let v = vec![\n            1,\n            2,\n        ];\n    }\n}\n";
    const TS: &str = "import a from 'a';\nimport b from 'b';\nimport c from 'c';\nimport d from 'd';\n\nclass Q {\n  run() {\n    const o = {\n      a: 1,\n      b: 2,\n    };\n    call(\n      1,\n      2,\n    );\n  }\n}\n";
    const PY: &str = "import a\nimport b\nfrom c import d\nimport e\n\ndef run():\n    if x:\n        one()\n        two()\n        three()\n    else:\n        four()\n        five()\n        six()\n    v = [\n        1,\n        2,\n    ]\n";
    const JAVA: &str = "import a.B;\nimport c.D;\nimport e.F;\nimport g.H;\n\nclass Q {\n  void run() {\n    int[] v = {\n      1,\n      2,\n    };\n  }\n}\n";
    const SWIFT: &str = "import A\nimport B\nimport C\nimport D\n\nstruct Q {\n  func run() {\n    let v = [\n      1,\n      2,\n    ]\n  }\n}\n";

    #[test]
    fn brackets_python_blocks_and_import_runs_fold_in_every_language() {
        // Imports; the type body; the method; `if` and `else` sharing the `} else {` line; a
        // multi-line literal or argument list.
        assert_eq!(
            fold_lines(Language::Rust, RUST),
            [(0, 3), (5, 19), (6, 18), (7, 10), (10, 13), (14, 17)]
        );
        let ts = [(0, 3), (5, 16), (6, 15), (7, 10), (11, 14)];
        assert_eq!(fold_lines(Language::TypeScript, TS), ts);
        assert_eq!(fold_lines(Language::JavaScript, TS), ts);
        // Python folds indented blocks from their header line, and its brackets.
        assert_eq!(
            fold_lines(Language::Python, PY),
            [(0, 3), (5, 17), (6, 9), (10, 13), (14, 17)]
        );
        assert_eq!(
            fold_lines(Language::Java, JAVA),
            [(0, 3), (5, 12), (6, 11), (7, 10)]
        );
        assert_eq!(
            fold_lines(Language::Swift, SWIFT),
            [(0, 3), (5, 12), (6, 11), (7, 10)]
        );
    }

    #[test]
    fn json_objects_and_arrays_fold_nested() {
        let src = "{\n  \"a\": {\n    \"b\": [\n      1,\n      2\n    ],\n    \"c\": 3\n  },\n  \"d\": [\n    {\"e\": 1},\n    {\n      \"f\": [1, 2],\n      \"g\": 2\n    }\n  ]\n}\n";
        assert_eq!(
            fold_lines(Language::Json, src),
            [(0, 15), (1, 7), (2, 5), (8, 14), (10, 13)]
        );
        let a = analyze(Language::Json, src).unwrap();
        assert!(!a.has_error);
        assert!(
            a.identifiers.is_empty() && a.calls.is_empty(),
            "highlight-only otherwise"
        );
    }

    #[test]
    fn one_line_json_objects_do_not_fold() {
        let src = "{\"a\": {\"b\": [1, 2, 3]}, \"c\": []}\n";
        let a = analyze(Language::Json, src).unwrap();
        assert_eq!(a.folds.len(), 4, "the regions exist");
        assert_eq!(fold_lines(Language::Json, src), []);
        // Three lines (one to hide) is under the viewer's minimum, as for code.
        assert_eq!(fold_lines(Language::Json, "{\n  \"a\": 1\n}\n"), []);
    }

    #[test]
    fn jsonc_comments_do_not_break_folds() {
        let src = "// settings\n{\n  /* the theme */\n  \"theme\": \"x\", // trailing\n  \"list\": [\n    1, // one\n    2\n  ]\n}\n";
        assert_eq!(fold_lines(Language::Json, src), [(1, 8), (4, 7)]);
        assert!(!analyze(Language::Json, src).unwrap().has_error);
    }

    #[test]
    fn a_lone_import_or_a_short_block_does_not_fold() {
        assert_eq!(
            fold_lines(Language::Rust, "use a::b;\nfn f() { x(); }\n"),
            []
        );
    }

    fn calls(language: Language, src: &str) -> Vec<&str> {
        let a = analyze(language, src).unwrap();
        a.calls.iter().map(|r| &src[r.clone()]).collect()
    }

    #[test]
    fn rust_calls() {
        let src = "fn f() { a(); x.b(1); m::c::<u8>(); println!(\"x\"); let n = N::new(); }";
        assert_eq!(
            calls(Language::Rust, src),
            ["a", "b", "c", "println", "new"]
        );
    }

    #[test]
    fn python_calls() {
        let src = "def f():\n    a()\n    x.b(1)\n    y.z.c()\n";
        assert_eq!(calls(Language::Python, src), ["a", "b", "c"]);
    }

    #[test]
    fn typescript_and_javascript_calls() {
        let src = "function f() { a(); x.b(1); new C(); q.r.s(); }";
        assert_eq!(calls(Language::TypeScript, src), ["a", "b", "C", "s"]);
        assert_eq!(calls(Language::JavaScript, src), ["a", "b", "C", "s"]);
        assert_eq!(calls(Language::Tsx, src), ["a", "b", "C", "s"]);
    }

    #[test]
    fn binding_sites_are_plain_names_only() {
        let src = "fn f(a: u8, (b, c): (u8, u8)) { let x = 1; let mut y = 2; let (p, q) = (1, 2); x + y }";
        let a = analyze(Language::Rust, src).unwrap();
        let got: Vec<&str> = a.bindings.iter().map(|r| &src[r.clone()]).collect();
        assert_eq!(got, ["a", "x", "y"]);
        let ts = "function f(a: number) { const b = 1; let { c } = o; }";
        let a = analyze(Language::TypeScript, ts).unwrap();
        let got: Vec<&str> = a.bindings.iter().map(|r| &ts[r.clone()]).collect();
        assert_eq!(got, ["a", "b"]);
    }

    #[test]
    fn java_calls_and_bindings() {
        let src = "class A { int n = 0, m; void f(int a, String... rest) { g(); x.h(1); new ArrayList<String>(); new java.util.Date(); int y = 1; for (T t : ts) {} try (R r = open()) {} catch (E e) {} run(v -> v); run((p, q) -> p); } }";
        assert_eq!(
            calls(Language::Java, src),
            ["g", "h", "ArrayList", "Date", "open", "run", "run"]
        );
        let a = analyze(Language::Java, src).unwrap();
        let got: Vec<&str> = a.bindings.iter().map(|r| &src[r.clone()]).collect();
        assert_eq!(
            got,
            ["n", "m", "a", "rest", "y", "t", "r", "e", "v", "p", "q"]
        );
    }

    #[test]
    fn swift_calls() {
        let src = "func f() { a(); x.b(1); y.z.c() }";
        assert_eq!(calls(Language::Swift, src), ["a", "b", "c"]);
    }

    #[test]
    fn identifiers_skip_comments_and_strings() {
        let src = "fn f() { let abc = \"abc\"; /* abc */ abc }";
        let a = analyze(Language::Rust, src).unwrap();
        let names: Vec<&str> = a.identifiers.iter().map(|r| &src[r.clone()]).collect();
        assert_eq!(names, ["f", "abc", "abc"]);
    }

    #[test]
    fn partial_parse_is_reported() {
        assert!(!analyze(Language::Rust, "fn f() {}").unwrap().has_error);
        assert!(analyze(Language::Rust, "fn f( {").unwrap().has_error);
    }
}
