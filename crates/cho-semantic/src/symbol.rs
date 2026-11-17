//! The header of a global SCIP symbol: `scheme manager name version descriptors`.
//! Parsed in one place so every reader agrees on where the package name is.

/// The five space-separated parts of a global SCIP symbol. The descriptors are kept as
/// written (they may contain spaces inside backticks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header<'a> {
    pub scheme: &'a str,
    /// `npm`, `cargo`, `pip`, … (or `.` when the indexer gave none).
    pub manager: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub descriptors: &'a str,
}

impl<'a> Header<'a> {
    /// `None` for locals (`local 3`) and anything with fewer than five parts.
    pub fn parse(symbol: &'a str) -> Option<Self> {
        let mut parts = symbol.splitn(5, ' ');
        Some(Self {
            scheme: parts.next()?,
            manager: parts.next()?,
            name: parts.next()?,
            version: parts.next()?,
            descriptors: parts.next()?,
        })
    }

    /// The package name, unless the indexer left it empty or `.` (no package).
    pub fn package(&self) -> Option<&'a str> {
        (!self.name.is_empty() && self.name != ".").then_some(self.name)
    }
}

/// The package name inside a global SCIP symbol; see [`Header::package`].
pub fn package_of(symbol: &str) -> Option<&str> {
    Header::parse(symbol)?.package()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_header() {
        let h = Header::parse("scip-typescript npm @acme/core 1.0.0 dist/`x.d.ts`/A#").unwrap();
        assert_eq!(
            (h.scheme, h.manager, h.name, h.version, h.descriptors),
            (
                "scip-typescript",
                "npm",
                "@acme/core",
                "1.0.0",
                "dist/`x.d.ts`/A#"
            )
        );
        assert_eq!(
            package_of("rust-analyzer cargo tokio 1.2.3 task/spawn()."),
            Some("tokio")
        );
        assert_eq!(package_of("t . . . a/b."), None);
        assert_eq!(package_of("local 3"), None);
        assert_eq!(Header::parse("local 3"), None);
    }
}
