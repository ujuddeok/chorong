//! npm version specs as the indexer-version settings take them: an exact version (`0.6.0`), a
//! caret or tilde range (`^0.6.0`, `~0.6.0`) or `latest`. Nothing else npm accepts (`>=`, `||`,
//! `x` ranges, tags other than `latest`) is allowed, so what a setting means is plain.

use std::cmp::Ordering;
use std::fmt;

/// A released version: `major.minor.patch`, with an optional pre-release (`-rc.1`). Build
/// metadata (`+…`) is kept but never compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: String,
    pub build: String,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let (rest, build) = text.split_once('+').unwrap_or((text, ""));
        let (core, pre) = rest.split_once('-').unwrap_or((rest, ""));
        let mut parts = core.split('.');
        let num = |p: Option<&str>| -> Option<u64> {
            let p = p?;
            // No leading zeros and digits only, as semver says.
            if p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
            {
                return None;
            }
            p.parse().ok()
        };
        let v = Version {
            major: num(parts.next())?,
            minor: num(parts.next())?,
            patch: num(parts.next())?,
            pre: pre.to_string(),
            build: build.to_string(),
        };
        let ident_ok = |s: &str| {
            s.split('.')
                .all(|i| !i.is_empty() && i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        };
        let ok = parts.next().is_none()
            && (pre.is_empty() || ident_ok(pre))
            && (build.is_empty() || ident_ok(build))
            && !(rest.contains('-') && pre.is_empty())
            && !(text.contains('+') && build.is_empty());
        ok.then_some(v)
    }

    fn cmp_pre(a: &str, b: &str) -> Ordering {
        match (a.is_empty(), b.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            _ => {}
        }
        let mut x = a.split('.');
        let mut y = b.split('.');
        loop {
            match (x.next(), y.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(p), Some(q)) => {
                    let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                        (Ok(m), Ok(n)) => m.cmp(&n),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => p.cmp(q),
                    };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
            }
        }
    }

    fn triple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.triple()
            .cmp(&other.triple())
            .then_with(|| Self::cmp_pre(&self.pre, &other.pre))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build)?;
        }
        Ok(())
    }
}

/// What an indexer-version setting asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    /// This version exactly.
    Exact(Version),
    /// `^x.y.z`: npm's caret, the same leftmost non-zero part.
    Caret(Version),
    /// `~x.y.z`: the same major and minor.
    Tilde(Version),
    /// The registry's `latest` tag.
    Latest,
}

impl Spec {
    /// Parses a setting's text; `Err` says what is allowed.
    pub fn parse(text: &str) -> Result<Self, String> {
        let t = text.trim();
        let bad = || {
            format!(
                "`{t}` is not a version: use an exact version (0.6.0), a range (^0.6.0 or ~0.6.0) or latest"
            )
        };
        if t == "latest" {
            return Ok(Spec::Latest);
        }
        let (make, rest): (fn(Version) -> Spec, &str) = if let Some(r) = t.strip_prefix('^') {
            (Spec::Caret, r)
        } else if let Some(r) = t.strip_prefix('~') {
            (Spec::Tilde, r)
        } else {
            (Spec::Exact, t)
        };
        // `v` prefixes are npm-legal but make a setting harder to read; build metadata never
        // decides a match.
        if rest.starts_with('v') || rest.contains('+') {
            return Err(bad());
        }
        Version::parse(rest).map(make).ok_or_else(bad)
    }

    /// True for a range or `latest`: what it names can change without the setting changing,
    /// so it is checked against the registry now and then.
    pub fn floats(&self) -> bool {
        !matches!(self, Spec::Exact(_))
    }

    /// Whether `v` satisfies this spec. `Latest` matches any release (the registry decides).
    /// Pre-releases match only a spec that names a pre-release of the same `x.y.z`, as in npm.
    pub fn matches(&self, v: &Version) -> bool {
        let base = match self {
            Spec::Latest => return v.pre.is_empty(),
            Spec::Exact(b) => return v.cmp(b) == Ordering::Equal,
            Spec::Caret(b) | Spec::Tilde(b) => b,
        };
        if !v.pre.is_empty() && !(base.triple() == v.triple() && !base.pre.is_empty()) {
            return false;
        }
        if v < base {
            return false;
        }
        let upper = match self {
            Spec::Caret(b) if b.major > 0 => (b.major + 1, 0, 0),
            Spec::Caret(b) if b.minor > 0 => (0, b.minor + 1, 0),
            Spec::Caret(b) => (0, 0, b.patch + 1),
            Spec::Tilde(b) => (b.major, b.minor + 1, 0),
            _ => unreachable!(),
        };
        v.triple() < upper
    }

    /// The best (highest) of `versions` that this spec matches.
    pub fn best<'a>(&self, versions: impl IntoIterator<Item = &'a Version>) -> Option<&'a Version> {
        versions.into_iter().filter(|v| self.matches(v)).max()
    }
}

impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Spec::Exact(v) => write!(f, "{v}"),
            Spec::Caret(v) => write!(f, "^{v}"),
            Spec::Tilde(v) => write!(f, "~{v}"),
            Spec::Latest => f.write_str("latest"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(v("0.6.0").to_string(), "0.6.0");
        assert_eq!(v("1.2.3-rc.1+b5").to_string(), "1.2.3-rc.1+b5");
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.x",
            "1.2.3-",
            "1.2.3+",
            "a.b.c",
            "1.2.3-r..1",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        assert!(v("1.0.0-alpha") < v("1.0.0-alpha.1"));
        assert!(v("1.0.0-alpha.1") < v("1.0.0-beta"));
        assert!(v("1.0.0-rc.1") < v("1.0.0"));
        assert!(v("0.6.10") > v("0.6.9"));
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
    }

    #[test]
    fn specs_parse_or_say_what_is_allowed() {
        assert_eq!(Spec::parse(" latest ").unwrap(), Spec::Latest);
        assert_eq!(Spec::parse("^0.6.0").unwrap(), Spec::Caret(v("0.6.0")));
        assert_eq!(Spec::parse("~1.2.3").unwrap(), Spec::Tilde(v("1.2.3")));
        assert_eq!(Spec::parse("0.6.8").unwrap(), Spec::Exact(v("0.6.8")));
        for bad in [
            ">=1.0.0",
            "1.x",
            "^1",
            "next",
            "1.0.0 || 2.0.0",
            "v1.0.0",
            "^v1.0.0",
            "1.0.0+b",
            "",
        ] {
            let e = Spec::parse(bad).unwrap_err();
            assert!(e.contains("^0.6.0"), "{bad}: {e}");
        }
        assert!(!Spec::parse("0.6.0").unwrap().floats());
        assert!(Spec::parse("~0.6.0").unwrap().floats());
        assert!(Spec::parse("latest").unwrap().floats());
    }

    #[test]
    fn ranges_match_like_npm() {
        let m = |spec: &str, ver: &str| Spec::parse(spec).unwrap().matches(&v(ver));
        assert!(m("^0.6.0", "0.6.9") && !m("^0.6.0", "0.7.0") && !m("^0.6.1", "0.6.0"));
        assert!(m("^1.2.0", "1.9.9") && !m("^1.2.0", "2.0.0"));
        assert!(m("^0.0.3", "0.0.3") && !m("^0.0.3", "0.0.4"));
        assert!(m("~1.2.3", "1.2.9") && !m("~1.2.3", "1.3.0"));
        assert!(m("~0.6.0", "0.6.4") && !m("~0.6.0", "0.7.0"));
        assert!(!m("^0.6.0", "0.6.5-rc.1"));
        assert!(m("^0.6.5-rc.1", "0.6.5-rc.2") && m("^0.6.5-rc.1", "0.6.5"));
        assert!(m("0.6.0", "0.6.0") && !m("0.6.0", "0.6.1"));
        assert!(m("latest", "9.0.0") && !m("latest", "9.0.0-rc.1"));
        let all = [v("0.5.9"), v("0.6.0"), v("0.6.3"), v("0.7.1")];
        assert_eq!(Spec::parse("^0.6.0").unwrap().best(&all), Some(&all[2]));
        assert_eq!(Spec::parse("^0.8.0").unwrap().best(&all), None);
    }
}
