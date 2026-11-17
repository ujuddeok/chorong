//! Definition peek for members with no outline entry: the enclosing declaration,
//! with the member's lines marked.

use cho_semantic::index::ScipIndex;
use cho_semantic::{DeclarationKind, IndexStatus, Location, NoDeclaration, Semantic};
use std::collections::HashMap;

/// Peeks the definition of `needle` (its first occurrence) in `source` written to
/// `name`; returns the kind, the declaration text and the marked text.
fn peek(
    name: &str,
    source: &str,
    needle: &str,
) -> Result<(DeclarationKind, String, Option<String>), NoDeclaration> {
    let dir = std::env::temp_dir().join(format!("cho-members-{}-{}", std::process::id(), name));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), source).unwrap();
    let sem = Semantic::new(
        ScipIndex::new(Vec::new(), HashMap::new()),
        &dir,
        IndexStatus::Current,
    );
    let start = source.find(needle).unwrap();
    let loc = Location {
        path: name.into(),
        range: start..start + needle.len(),
        line: 1,
        column: 1,
        is_definition: true,
        mapped: None,
    };
    let result = sem.peek_definition(&loc).map(|p| {
        (
            p.kind(),
            source[p.range().clone()].to_string(),
            p.marked_range().map(|m| {
                assert!(p.range().start <= m.start && m.end <= p.range().end);
                source[m.clone()].to_string()
            }),
        )
    });
    std::fs::remove_dir_all(dir).ok();
    result
}

#[test]
fn rust_field_and_parameter() {
    let src = "pub struct P {\n    pub x: i32,\n    pub y: i32,\n}\n\nfn f(a: u8,\n      b: u8) -> u8 {\n    a + b\n}\n";
    let (k, decl, marked) = peek("a.rs", src, "y: i32").unwrap();
    assert_eq!(k, DeclarationKind::Enclosing);
    assert_eq!(decl, "pub struct P {\n    pub x: i32,\n    pub y: i32,\n}");
    assert_eq!(marked.unwrap(), "    pub y: i32,");
    let (k, decl, marked) = peek("a.rs", src, "b: u8").unwrap();
    assert_eq!(k, DeclarationKind::Enclosing);
    assert!(decl.starts_with("fn f(a: u8,") && decl.ends_with('}'));
    assert_eq!(marked.unwrap(), "      b: u8) -> u8 {");
}

#[test]
fn a_symbols_own_declaration_stays_a_declaration() {
    let src = "pub struct P {\n    pub x: i32,\n}\n";
    let (k, decl, marked) = peek("b.rs", src, "P").unwrap();
    assert_eq!(k, DeclarationKind::Declaration);
    assert_eq!(decl, src.trim_end());
    assert_eq!(marked, None);
}

#[test]
fn typescript_property_field_and_arrow_parameter() {
    let src = "interface Money {\n  cents: number;\n}\nclass Svc {\n  private tax = 2;\n  run(): void {}\n}\nconst twice = (n: number) => n * 2;\n";
    let (k, decl, marked) = peek("m.ts", src, "cents").unwrap();
    assert_eq!(k, DeclarationKind::Enclosing);
    assert_eq!(decl, "interface Money {\n  cents: number;\n}");
    assert_eq!(marked.unwrap(), "  cents: number;");
    let (_, decl, marked) = peek("m.ts", src, "tax").unwrap();
    assert!(decl.starts_with("class Svc {"), "{decl}");
    assert_eq!(marked.unwrap(), "  private tax = 2;");
    let (k, decl, marked) = peek("m.ts", src, "n: number").unwrap();
    assert_eq!(k, DeclarationKind::Enclosing);
    assert!(
        decl.starts_with("twice = ") || decl.starts_with("const twice"),
        "{decl}"
    );
    assert_eq!(
        marked.unwrap(),
        "twice = (n: number) => n * 2",
        "clamped to the declarator"
    );
}

#[test]
fn python_attribute_and_parameter() {
    let src = "class C:\n    size = 3\n\n    def m(self, k):\n        return k\n";
    let (k, decl, marked) = peek("c.py", src, "size").unwrap();
    assert_eq!(k, DeclarationKind::Enclosing);
    assert!(decl.starts_with("class C:"));
    assert_eq!(marked.unwrap(), "    size = 3");
    let (_, decl, marked) = peek("c.py", src, "k)").unwrap();
    assert!(decl.starts_with("def m(self, k):"), "{decl}");
    assert_eq!(marked.unwrap(), "def m(self, k):");
}

#[test]
fn a_local_marks_its_line_in_the_enclosing_function() {
    let src = "fn g() -> i32 {\n    let total = 1;\n    total\n}\n";
    let (k, decl, marked) = peek("g.rs", src, "total").unwrap();
    assert_eq!(k, DeclarationKind::Enclosing);
    assert_eq!(decl, src.trim_end());
    assert_eq!(marked.unwrap(), "    let total = 1;");
}

#[test]
fn nothing_enclosing_is_no_declaration() {
    assert_eq!(
        peek("s.py", "top = 1\nprint(top)\n", "top"),
        Err(NoDeclaration::NoDeclaration)
    );
}
