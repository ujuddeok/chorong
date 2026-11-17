//! Hover information: what the index recorded about a symbol (signature,
//! documentation, where it is defined) and which index said so.
//!
//! Nothing is synthesised. The signature is the indexer's own text; when the
//! index has none, [`Hover::signature`] is the [`NoSignature`] that says why. See `docs/public/semantic.md`, "Hover information".

use crate::cache::IndexStatus;
use crate::index::SymbolId;
use crate::query::{Location, Semantic};
use serde::{Serialize, Serializer};
use std::ops::Range;

/// Where a signature's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureSource {
    /// SCIP `signature_documentation` (rust-analyzer, and the SCIP spec's intended place).
    SignatureDocumentation,
    /// The first `documentation` entry, when that entry is exactly one fenced code
    /// block. scip-typescript records its hover signature this way and leaves
    /// `signature_documentation` empty.
    Documentation,
}

/// A signature as the indexer recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HoverSignature {
    pub text: String,
    /// The language the indexer gave, e.g. `rust` or `ts`; may be empty.
    pub language: String,
    pub source: SignatureSource,
}

/// Why a hover has no signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoSignature {
    /// The index has occurrences of the symbol but no symbol information for it
    /// (common for symbols from dependencies and the standard library).
    NoSymbolInformation,
    /// The index has symbol information but the indexer recorded no signature.
    NotRecorded,
}

impl NoSignature {
    pub fn as_str(self) -> &'static str {
        match self {
            NoSignature::NoSymbolInformation => "no-symbol-information",
            NoSignature::NotRecorded => "not-recorded",
        }
    }
}

/// The index an answer came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HoverIndex {
    /// The commit the index was built at; `None` without Git, or when the answer
    /// can't be tied to one project and the projects' commits differ.
    pub commit: Option<String>,
    pub status: IndexStatus,
}

/// Why a definition has no declaration range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoDeclaration {
    /// The file isn't under the repository root or can't be read.
    OutsideIndexedFiles,
    /// No declaration in the file's outline is at or contains the definition.
    NoDeclaration,
    /// Chorong has no Tree-sitter grammar for the file.
    UnsupportedLanguage,
}

impl NoDeclaration {
    pub fn as_str(self) -> &'static str {
        match self {
            NoDeclaration::OutsideIndexedFiles => "outside-indexed-files",
            NoDeclaration::NoDeclaration => "no-declaration",
            NoDeclaration::UnsupportedLanguage => "unsupported-language",
        }
    }
}

/// Which declaration a peek shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationKind {
    /// The symbol's own declaration.
    Declaration,
    /// The declaration that contains the symbol, which is a field, property,
    /// parameter or local with no outline entry of its own.
    Enclosing,
}

impl DeclarationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DeclarationKind::Declaration => "declaration",
            DeclarationKind::Enclosing => "enclosing",
        }
    }
}

/// The result of a definition peek: the whole declaration (signature and body) in the
/// defining file, as byte ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationPeek {
    /// The symbol's own declaration.
    Declaration { range: Range<usize> },
    /// The declaration that contains the symbol (see [`DeclarationKind::Enclosing`]), with
    /// the byte range of the member's line(s), inside `range`.
    Enclosing {
        range: Range<usize>,
        marked: Range<usize>,
    },
}

impl DeclarationPeek {
    pub fn kind(&self) -> DeclarationKind {
        match self {
            DeclarationPeek::Declaration { .. } => DeclarationKind::Declaration,
            DeclarationPeek::Enclosing { .. } => DeclarationKind::Enclosing,
        }
    }

    /// The declaration's full byte range.
    pub fn range(&self) -> &Range<usize> {
        match self {
            DeclarationPeek::Declaration { range } | DeclarationPeek::Enclosing { range, .. } => {
                range
            }
        }
    }

    /// The member's marked line(s), for an enclosing declaration.
    pub fn marked_range(&self) -> Option<&Range<usize>> {
        match self {
            DeclarationPeek::Declaration { .. } => None,
            DeclarationPeek::Enclosing { marked, .. } => Some(marked),
        }
    }
}

/// A definition location, optionally with its declaration's full byte range.
///
/// JSON: the location's fields, then `declaration_range`, `declaration_kind`,
/// `marked_range` and `no_declaration` (each `null` when it doesn't apply).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HoverDefinition {
    #[serde(flatten)]
    pub location: Location,
    /// Set only by the peek calls: the declaration, or why none was found.
    #[serde(flatten, serialize_with = "peek_fields")]
    pub peek: Option<Result<DeclarationPeek, NoDeclaration>>,
}

impl HoverDefinition {
    /// The declaration a peek found.
    pub fn declaration(&self) -> Option<&DeclarationPeek> {
        self.peek.as_ref()?.as_ref().ok()
    }

    /// Why a peek found no declaration.
    pub fn no_declaration(&self) -> Option<NoDeclaration> {
        self.peek.as_ref()?.as_ref().err().copied()
    }
}

/// [`HoverDefinition::peek`] as the four flat fields of the JSON shape.
fn peek_fields<S: Serializer>(
    peek: &Option<Result<DeclarationPeek, NoDeclaration>>,
    s: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Fields<'a> {
        declaration_range: Option<&'a Range<usize>>,
        declaration_kind: Option<DeclarationKind>,
        marked_range: Option<&'a Range<usize>>,
        no_declaration: Option<NoDeclaration>,
    }
    let found = peek.as_ref().and_then(|p| p.as_ref().ok());
    Fields {
        declaration_range: found.map(DeclarationPeek::range),
        declaration_kind: found.map(DeclarationPeek::kind),
        marked_range: found.and_then(DeclarationPeek::marked_range),
        no_declaration: peek.as_ref().and_then(|p| p.as_ref().err().copied()),
    }
    .serialize(s)
}

/// [`Hover::signature`] as the `signature` and `no_signature` fields of the JSON shape.
fn signature_fields<S: Serializer>(
    signature: &Result<HoverSignature, NoSignature>,
    s: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Fields<'a> {
        signature: Option<&'a HoverSignature>,
        no_signature: Option<NoSignature>,
    }
    Fields {
        signature: signature.as_ref().ok(),
        no_signature: signature.as_ref().err().copied(),
    }
    .serialize(s)
}

/// What the index knows about one symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hover {
    #[serde(flatten)]
    pub id: SymbolId,
    /// The hovered occurrence's byte range ([`Semantic::hover_at`] only).
    pub range: Option<Range<usize>>,
    /// The symbol's canonical name: the index's display name or, when that is empty,
    /// the last descriptor of the symbol ID (`…/money.ts/add().` gives `add`). Locals
    /// (`local N`) have no name in the ID, so they use the source text. Empty only
    /// when none of these is available.
    pub name: String,
    /// The source text the hovered occurrence covers ([`Semantic::hover_at`] only;
    /// empty for [`Semantic::hover`]). Differs from `name` when code renames, e.g.
    /// `import { add as plus }`.
    pub local_text: String,
    /// The SCIP kind, e.g. `Function`; `UnspecifiedKind` or empty when unknown.
    pub kind: String,
    /// The indexer's signature, or why there is none. JSON: `signature` and
    /// `no_signature`, one of them `null`.
    #[serde(flatten, serialize_with = "signature_fields")]
    pub signature: Result<HoverSignature, NoSignature>,
    /// Markdown, as the indexer wrote it, minus an entry used as the signature.
    pub documentation: Vec<String>,
    /// Where the symbol is defined; empty for symbols defined outside the index.
    /// Declaration ranges are filled only by the peek calls.
    pub definitions: Vec<HoverDefinition>,
    pub index: HoverIndex,
    /// Set when the symbol is a built declaration (`dist/x.d.ts`) mapped to its source:
    /// the signature, documentation and definitions are the source symbol's. Unverified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapped: Option<crate::MappedFrom>,
}

impl Semantic {
    /// Hover information for the symbol under `byte` in `path`. `None` when no
    /// symbol is there. Costs a [`Semantic::symbol_at`] plus the symbol's
    /// definitions, all from in-memory tables, so it can run on every pointer move.
    pub fn hover_at(&self, path: &str, byte: usize) -> Option<Hover> {
        let at = self.symbol_at(path, byte)?;
        let mut hover = self.hover(&at.id)?;
        hover.local_text = self.source_text(path, at.range.clone());
        if hover.name.is_empty() {
            hover.name = hover.local_text.clone();
        }
        hover.index = self.index_info(Some(path));
        hover.range = Some(at.range);
        Some(hover)
    }

    /// [`Semantic::hover_at`] plus each definition's declaration range. Parses the
    /// defining files on a cache miss, so call it only when a card or shortcut asks,
    /// and off the UI thread.
    pub fn hover_at_with_peek(&self, path: &str, byte: usize) -> Option<Hover> {
        let mut hover = self.hover_at(path, byte)?;
        self.peek(&mut hover);
        Some(hover)
    }

    /// Fills [`HoverDefinition::peek`] on every definition of `hover`.
    pub fn peek(&self, hover: &mut Hover) {
        for d in &mut hover.definitions {
            d.peek = Some(self.peek_definition(&d.location));
        }
    }

    /// Hover information for `symbol`. `None` when the index has neither symbol
    /// information nor occurrences for it.
    pub fn hover(&self, symbol: &SymbolId) -> Option<Hover> {
        let own = self.info(symbol);
        if own.is_none() && self.index().occurrences_of(symbol).is_none() {
            return None;
        }
        // A mapped built declaration shows its source: the declaration the user reads.
        let source = self.mapped_source(symbol);
        let info = source.and_then(|(s, _)| self.info(s)).or(own);
        let mapped = source.map(|(s, mapping)| crate::MappedFrom {
            symbol: s.symbol.clone(),
            mapping,
            verified: false,
        });
        let definitions = self.definitions(symbol);
        let mut documentation = info.map(|i| i.documentation.clone()).unwrap_or_default();
        let signature = match info {
            None => Err(NoSignature::NoSymbolInformation),
            Some(i) => match &i.signature {
                Some(sig) => Ok(HoverSignature {
                    text: sig.text.clone(),
                    language: sig.language.clone(),
                    source: SignatureSource::SignatureDocumentation,
                }),
                None => documentation
                    .first()
                    .and_then(|d| fenced_block(d))
                    .map(|(language, text)| HoverSignature {
                        text,
                        language,
                        source: SignatureSource::Documentation,
                    })
                    .ok_or(NoSignature::NotRecorded),
            },
        };
        if signature
            .as_ref()
            .is_ok_and(|s| s.source == SignatureSource::Documentation)
        {
            documentation.remove(0);
        }
        let mut name = info.map(|i| i.display_name.clone()).unwrap_or_default();
        if name.is_empty() {
            name = symbol_name(&symbol.symbol).unwrap_or_default();
        }
        if name.is_empty()
            && let Some(d) = definitions.first()
        {
            name = self.source_text(&d.path, d.range.clone());
        }
        let index = self.index_info(definitions.first().map(|d| d.path.as_str()));
        Some(Hover {
            id: symbol.clone(),
            range: None,
            name,
            local_text: String::new(),
            kind: info.map(|i| i.kind.clone()).unwrap_or_default(),
            signature,
            documentation,
            definitions: definitions
                .into_iter()
                .map(|location| HoverDefinition {
                    location,
                    peek: None,
                })
                .collect(),
            index,
            mapped,
        })
    }

    /// The text `range` covers in the current file, or empty.
    fn source_text(&self, path: &str, range: Range<usize>) -> String {
        self.buffer(path)
            .and_then(|b| b.text().get(range).map(str::to_string))
            .unwrap_or_default()
    }

    /// The index that describes `path`: the project with the longest root that
    /// contains it. Without a path, or a matching project, the overall status and
    /// the projects' commit if they all agree.
    pub fn index_info(&self, path: Option<&str>) -> HoverIndex {
        let contains =
            |root: &str, p: &str| root.is_empty() || crate::relpath::below(p, root).is_some();
        let best = path.and_then(|p| {
            self.origins()
                .iter()
                .filter(|o| contains(&o.project_root, p))
                .max_by_key(|o| o.project_root.len())
        });
        if let Some(o) = best {
            return HoverIndex {
                commit: o.commit.clone(),
                status: o.status,
            };
        }
        let mut commits = self.origins().iter().map(|o| &o.commit);
        let first = commits.next().cloned().flatten();
        let agreed = commits.all(|c| *c == first);
        HoverIndex {
            commit: if agreed { first } else { None },
            status: self.status(),
        }
    }
}

/// The last descriptor's name of a global SCIP symbol, parsed with the `scip`
/// crate's grammar: backtick-escaped names are unescaped and every suffix
/// (`/` `#` `.` `:` `!` `()` `[]` and parameters) is handled. `None` for locals and
/// for symbols that don't parse.
pub fn symbol_name(symbol: &str) -> Option<String> {
    if scip::symbol::is_local_symbol(symbol) {
        return None;
    }
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let name = parsed.descriptors.last()?.name.clone();
    (!name.is_empty()).then_some(name)
}

/// `(language, body)` when `doc` is exactly one fenced code block (```` ```lang ````
/// … ```` ``` ````), as scip-typescript writes its hover signature. Anything else,
/// including prose around the block, is not a signature.
fn fenced_block(doc: &str) -> Option<(String, String)> {
    let doc = doc.trim();
    let rest = doc.strip_prefix("```")?;
    let (lang, body) = rest.split_once('\n')?;
    let body = body.strip_suffix("```")?;
    let body = body.strip_suffix('\n').unwrap_or(body);
    if lang.contains('`') || body.contains("```") || body.trim().is_empty() {
        return None;
    }
    Some((lang.trim().to_string(), body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cho hover --json` and other readers see the flat fields earlier versions printed.
    #[test]
    fn json_keeps_the_flat_fields() {
        let location = Location {
            path: "a.rs".into(),
            range: 3..5,
            line: 1,
            column: 4,
            is_definition: true,
            mapped: None,
        };
        let def = |peek| {
            serde_json::to_string(&HoverDefinition {
                location: location.clone(),
                peek,
            })
            .unwrap()
        };
        let loc =
            r#""path":"a.rs","range":{"start":3,"end":5},"line":1,"column":4,"is_definition":true"#;
        assert_eq!(
            def(None),
            format!(
                r#"{{{loc},"declaration_range":null,"declaration_kind":null,"marked_range":null,"no_declaration":null}}"#
            )
        );
        assert_eq!(
            def(Some(Ok(DeclarationPeek::Enclosing {
                range: 0..9,
                marked: 2..4
            }))),
            format!(
                r#"{{{loc},"declaration_range":{{"start":0,"end":9}},"declaration_kind":"enclosing","marked_range":{{"start":2,"end":4}},"no_declaration":null}}"#
            )
        );
        assert_eq!(
            def(Some(Ok(DeclarationPeek::Declaration { range: 0..9 }))),
            format!(
                r#"{{{loc},"declaration_range":{{"start":0,"end":9}},"declaration_kind":"declaration","marked_range":null,"no_declaration":null}}"#
            )
        );
        assert_eq!(
            def(Some(Err(NoDeclaration::UnsupportedLanguage))),
            format!(
                r#"{{{loc},"declaration_range":null,"declaration_kind":null,"marked_range":null,"no_declaration":"unsupported-language"}}"#
            )
        );
        let hover = |signature| {
            let h = Hover {
                id: SymbolId::new("s", ""),
                range: None,
                name: "n".into(),
                local_text: String::new(),
                kind: "Function".into(),
                signature,
                documentation: vec![],
                definitions: vec![],
                index: HoverIndex {
                    commit: None,
                    status: IndexStatus::Current,
                },
                mapped: None,
            };
            serde_json::to_string(&h).unwrap()
        };
        let head = r#""symbol":"s","local_in":null,"range":null,"name":"n","local_text":"","kind":"Function""#;
        let tail =
            r#""documentation":[],"definitions":[],"index":{"commit":null,"status":"current"}"#;
        assert_eq!(
            hover(Err(NoSignature::NotRecorded)),
            format!(r#"{{{head},"signature":null,"no_signature":"not-recorded",{tail}}}"#)
        );
        assert_eq!(
            hover(Ok(HoverSignature {
                text: "fn n()".into(),
                language: "rust".into(),
                source: SignatureSource::SignatureDocumentation,
            })),
            format!(
                r#"{{{head},"signature":{{"text":"fn n()","language":"rust","source":"signature-documentation"}},"no_signature":null,{tail}}}"#
            )
        );
    }

    #[test]
    fn only_a_lone_fenced_block_is_a_signature() {
        assert_eq!(
            fenced_block("```ts\nfunction f(): void\n```"),
            Some(("ts".into(), "function f(): void".into()))
        );
        assert_eq!(
            fenced_block("```\nlet x\n```\n"),
            Some((String::new(), "let x".into()))
        );
        assert_eq!(fenced_block("Adds two numbers."), None);
        assert_eq!(fenced_block("```ts\nf()\n```\nAnd prose."), None);
        assert_eq!(fenced_block("```ts\n```"), None);
        assert_eq!(fenced_block("```ts\na\n```\n```ts\nb\n```"), None);
    }

    #[test]
    fn names_come_from_the_last_descriptor() {
        let name = |s: &str| symbol_name(s);
        assert_eq!(name("t npm web 1 src/`m.ts`/add()."), Some("add".into()));
        assert_eq!(name("t npm web 1 src/`m.ts`/add(+1)."), Some("add".into()));
        assert_eq!(name("t npm web 1 src/`m.ts`/Money#"), Some("Money".into()));
        assert_eq!(name("t npm web 1 Money#cents."), Some("cents".into()));
        assert_eq!(name("t npm web 1 Money#total()."), Some("total".into()));
        assert_eq!(name("t npm web 1 f().(a)"), Some("a".into()));
        assert_eq!(name("t npm web 1 Box#[T]"), Some("T".into()));
        assert_eq!(name("t npm web 1 src/m/"), Some("m".into()));
        assert_eq!(name("t npm web 1 meta:"), Some("meta".into()));
        assert_eq!(name("t cargo p 1 vec!"), Some("vec".into()));
        assert_eq!(name("t npm web 1 a/`we ird`."), Some("we ird".into()));
        assert_eq!(name("t npm web 1 a/`has``tick`#"), Some("has`tick".into()));
        assert_eq!(name("t . . . a/b."), Some("b".into()));
        assert_eq!(name("local 12"), None);
        assert_eq!(name("not a symbol"), None);
        assert_eq!(name(""), None);
    }
}
