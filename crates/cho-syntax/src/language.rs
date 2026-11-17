use std::path::Path;

/// A language Chorong parses. Anything else is shown as plain text.
///
/// The data and markup formats (JSON, TOML, XML, Markdown) are highlight-only: they have no outline, call
/// analysis, reference kinds or folds (see [`Language::has_structure`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Swift,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Java,
    Json,
    Toml,
    Xml,
    Markdown,
}

impl Language {
    /// The programming languages, whose declarations and calls Chorong reads. Their order is
    /// fixed: the fuzz corpora's first byte picks one by index.
    pub const ALL: [Language; 7] = [
        Language::Rust,
        Language::Swift,
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Python,
        Language::Java,
    ];

    /// The highlight-only data and markup formats.
    pub const DATA: [Language; 4] = [
        Language::Json,
        Language::Toml,
        Language::Xml,
        Language::Markdown,
    ];

    /// Whether Chorong reads structure (declarations, calls, references, folds) from this
    /// language. `false` for the highlight-only data formats.
    pub fn has_structure(self) -> bool {
        !matches!(self, Self::Json | Self::Toml | Self::Xml | Self::Markdown)
    }

    /// Picks a language from a file's extension, or from its whole name for files such as
    /// `Cargo.lock`.
    pub fn from_path(path: &Path) -> Option<Self> {
        if path.file_name()? == "Cargo.lock" {
            return Some(Self::Toml);
        }
        match path.extension()?.to_str()? {
            "rs" => Some(Self::Rust),
            "swift" => Some(Self::Swift),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "py" | "pyi" => Some(Self::Python),
            "java" => Some(Self::Java),
            "json" | "jsonc" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "md" | "markdown" => Some(Self::Markdown),
            "xml" | "plist" | "svg" | "xaml" | "xsd" | "xsl" | "xslt" | "csproj" => Some(Self::Xml),
            _ => None,
        }
    }

    /// The language a Markdown code fence names (```` ```rust ````), from its first word.
    pub(crate) fn from_fence(name: &str) -> Option<Self> {
        FENCE_NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, l)| *l)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Swift => "Swift",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::JavaScript => "JavaScript",
            Self::Python => "Python",
            Self::Java => "Java",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Xml => "XML",
            Self::Markdown => "Markdown",
        }
    }

    pub(crate) fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Swift => tree_sitter_swift::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
            // The block grammar; inline text is injected (see `MARKDOWN_INJECTIONS`).
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }

    /// Highlight, injection and locals queries. TypeScript's queries extend
    /// JavaScript's, and the TypeScript patterns must come first so they win.
    pub(crate) fn queries(self) -> (String, &'static str, &'static str) {
        use tree_sitter_javascript as js;
        use tree_sitter_typescript as ts;
        match self {
            Self::Rust => (
                tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
                tree_sitter_rust::INJECTIONS_QUERY,
                "",
            ),
            Self::Swift => (
                tree_sitter_swift::HIGHLIGHTS_QUERY.into(),
                "",
                tree_sitter_swift::LOCALS_QUERY,
            ),
            Self::TypeScript => (
                [ts::HIGHLIGHTS_QUERY, js::HIGHLIGHT_QUERY].join("\n"),
                "",
                ts::LOCALS_QUERY,
            ),
            Self::Tsx => (
                [
                    ts::HIGHLIGHTS_QUERY,
                    js::JSX_HIGHLIGHT_QUERY,
                    js::HIGHLIGHT_QUERY,
                ]
                .join("\n"),
                "",
                ts::LOCALS_QUERY,
            ),
            Self::JavaScript => (
                [js::JSX_HIGHLIGHT_QUERY, js::HIGHLIGHT_QUERY].join("\n"),
                "",
                js::LOCALS_QUERY,
            ),
            Self::Python => (tree_sitter_python::HIGHLIGHTS_QUERY.into(), "", ""),
            // The bundled Java query leaves out records, annotation types and enum constants;
            // Chorong's additions go last: they must override its catch-all `(identifier) @variable`.
            Self::Java => (
                [tree_sitter_java::HIGHLIGHTS_QUERY, JAVA_EXTRA_HIGHLIGHTS].join("\n"),
                "",
                "",
            ),
            Self::Json => (JSON_HIGHLIGHTS.into(), "", ""),
            Self::Toml => (TOML_HIGHLIGHTS.into(), "", ""),
            Self::Xml => (tree_sitter_xml::XML_HIGHLIGHT_QUERY.into(), "", ""),
            Self::Markdown => (markdown_highlights(), MARKDOWN_INJECTIONS, ""),
        }
    }
}

/// Markdown's injections: inline text (emphasis, code spans, links) parsed by the inline
/// grammar, and fenced code by the language its info string names (see [`FENCE_NAMES`]). Both
/// nodes have hidden children, so without `include-children` the injected text has gaps.
/// The bundled query also injects HTML and YAML/TOML front matter; those stay one colour.
const MARKDOWN_INJECTIONS: &str = r#"
((inline) @injection.content
  (#set! injection.language "markdown_inline")
  (#set! injection.include-children))
(fenced_code_block
  (info_string (language) @injection.language)
  (code_fence_content) @injection.content
  (#set! injection.include-children))
"#;

/// Code-fence names Chorong highlights, lower case as usually written.
const FENCE_NAMES: &[(&str, Language)] = &[
    ("rust", Language::Rust),
    ("rs", Language::Rust),
    ("swift", Language::Swift),
    ("typescript", Language::TypeScript),
    ("ts", Language::TypeScript),
    ("tsx", Language::Tsx),
    ("javascript", Language::JavaScript),
    ("js", Language::JavaScript),
    ("jsx", Language::JavaScript),
    ("mjs", Language::JavaScript),
    ("cjs", Language::JavaScript),
    ("python", Language::Python),
    ("py", Language::Python),
    ("python3", Language::Python),
    ("java", Language::Java),
    ("json", Language::Json),
    ("jsonc", Language::Json),
    ("toml", Language::Toml),
    ("xml", Language::Xml),
    ("svg", Language::Xml),
    ("markdown", Language::Markdown),
    ("md", Language::Markdown),
];

/// Markdown block highlights: the bundled query (from nvim-treesitter, MIT), except that a
/// fenced block's code is `text.literal` only when its language isn't one Chorong highlights.
fn markdown_highlights() -> String {
    let names: Vec<String> = FENCE_NAMES.iter().map(|(n, _)| format!("{n:?}")).collect();
    format!(
        r#"
(atx_heading (inline) @text.title)
(setext_heading (paragraph) @text.title)
[
  (atx_h1_marker) (atx_h2_marker) (atx_h3_marker) (atx_h4_marker) (atx_h5_marker)
  (atx_h6_marker) (setext_h1_underline) (setext_h2_underline)
] @punctuation.special
[(link_title) (indented_code_block)] @text.literal
(fenced_code_block_delimiter) @punctuation.delimiter
(info_string) @label
(fenced_code_block !info_string (code_fence_content) @text.literal)
(fenced_code_block
  (info_string (language) @_language)
  (code_fence_content) @text.literal
  (#not-any-of? @_language {}))
(link_destination) @text.uri
(link_label) @text.reference
[
  (list_marker_plus) (list_marker_minus) (list_marker_star) (list_marker_dot)
  (list_marker_parenthesis) (thematic_break)
] @punctuation.special
[(block_continuation) (block_quote_marker)] @punctuation.special
(backslash_escape) @string.escape
"#,
        names.join(" ")
    )
}

/// The name [`MARKDOWN_INJECTIONS`] gives the inline grammar.
pub(crate) const MARKDOWN_INLINE: &str = "markdown_inline";

/// JSON, with keys as properties: the bundled query's `string.special.key` would draw them
/// in the same colour as the values. The key pattern goes last: of two patterns capturing
/// the same node, the later one is drawn (as with the Java additions above).
const JSON_HIGHLIGHTS: &str = r#"
(string) @string
(escape_sequence) @string.escape
(number) @number
[(null) (true) (false)] @constant.builtin
(comment) @comment
["," ":"] @punctuation.delimiter
["[" "]" "{" "}"] @punctuation.bracket
(pair key: (_) @property)
"#;

/// TOML, with table names as types and keys as properties. The bundled query captures every
/// bare key as a type first, so pair keys never get their own colour.
const TOML_HIGHLIGHTS: &str = r#"
(table [(bare_key) (quoted_key)] @type)
(table (dotted_key [(bare_key) (quoted_key)] @type))
(table_array_element [(bare_key) (quoted_key)] @type)
(table_array_element (dotted_key [(bare_key) (quoted_key)] @type))
(pair [(bare_key) (quoted_key)] @property)
(pair (dotted_key [(bare_key) (quoted_key)] @property))
(boolean) @boolean
(comment) @comment
(escape_sequence) @string.escape
(string) @string
[(integer) (float)] @number
[(offset_date_time) (local_date_time) (local_date) (local_time)] @string.special
["." ","] @punctuation.delimiter
"=" @operator
["[" "]" "[[" "]]" "{" "}"] @punctuation.bracket
"#;

/// Java captures missing from `tree-sitter-java`'s own query.
const JAVA_EXTRA_HIGHLIGHTS: &str = r#"
(annotation_type_declaration name: (identifier) @type)
(record_declaration name: (identifier) @type)
(enum_constant name: (identifier) @constant)
(annotation_type_element_declaration name: (identifier) @function.method)
(compact_constructor_declaration name: (identifier) @type)
(method_reference (identifier) @function.method .)
"@interface" @keyword
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(
            Language::from_path(Path::new("a/b.rs")),
            Some(Language::Rust)
        );
        assert_eq!(Language::from_path(Path::new("x.tsx")), Some(Language::Tsx));
        assert_eq!(
            Language::from_path(Path::new("x.mjs")),
            Some(Language::JavaScript)
        );
        assert_eq!(
            Language::from_path(Path::new("x.pyi")),
            Some(Language::Python)
        );
        assert_eq!(
            Language::from_path(Path::new("src/Main.java")),
            Some(Language::Java)
        );
        assert_eq!(Language::from_path(Path::new("Makefile")), None);
        assert_eq!(Language::from_path(Path::new("x.go")), None);
        for (path, language) in [
            ("package.json", Language::Json),
            ("tsconfig.jsonc", Language::Json),
            ("Cargo.toml", Language::Toml),
            ("Cargo.lock", Language::Toml),
            ("a/b/Cargo.lock", Language::Toml),
            ("pom.xml", Language::Xml),
            ("Info.plist", Language::Xml),
            ("icon.svg", Language::Xml),
            ("App.xaml", Language::Xml),
            ("README.md", Language::Markdown),
            ("notes.markdown", Language::Markdown),
        ] {
            assert_eq!(
                Language::from_path(Path::new(path)),
                Some(language),
                "{path}"
            );
            assert!(!language.has_structure(), "{path}");
        }
        assert_eq!(Language::from_path(Path::new("other.lock")), None);
        assert!(Language::ALL.iter().all(|l| l.has_structure()));
    }
}
