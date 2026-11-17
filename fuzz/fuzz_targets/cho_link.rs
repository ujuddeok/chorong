#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use cho_core::{LinkSide, ChorongLink, RepoRef};

#[derive(Arbitrary, Debug)]
struct OpenInput {
    kind: u8,
    a: String,
    b: String,
    c: String,
    rev: Option<String>,
    path: Option<String>,
    symbol: Option<String>,
    line: Option<usize>,
    branch: Option<String>,
    dirty: bool,
    pr: Option<u64>,
    base: Option<String>,
    /// 0 none, 1 before, 2 after
    side: u8,
}

#[derive(Arbitrary, Debug)]
enum Input {
    Raw(String),
    Open(OpenInput),
    Investigation(String),
}

fn check_parse(s: &str) {
    if let Ok(link) = ChorongLink::parse(s) {
        let url = link.to_url();
        let back = ChorongLink::parse(&url)
            .unwrap_or_else(|e| panic!("to_url output {url:?} failed to re-parse: {e}"));
        assert_eq!(back, link, "url {url:?}");
    }
}

fuzz_target!(|input: Input| {
    match input {
        Input::Raw(s) => check_parse(&s),
        Input::Investigation(src) => {
            let link = ChorongLink::Investigation { src };
            if let Ok(back) = ChorongLink::parse(&link.to_url()) {
                assert_eq!(back, link);
            }
        }
        Input::Open(o) => {
            let repo = match o.kind % 4 {
                0 => RepoRef::GitHub { owner: o.a, name: o.b },
                1 => RepoRef::Bitbucket { workspace: o.a, name: o.b },
                2 => RepoRef::BitbucketDc { host: o.a, project: o.b, name: o.c },
                _ => RepoRef::Local { path: o.a },
            };
            let link = ChorongLink::Open {
                repo,
                pr: o.pr,
                rev: o.rev,
                base: o.base.map(|b| b.to_ascii_lowercase()),
                side: match o.side % 3 {
                    0 => None,
                    1 => Some(LinkSide::Before),
                    _ => Some(LinkSide::After),
                },
                path: o.path,
                symbol: o.symbol,
                line: o.line,
                branch: o.branch,
                dirty: o.dirty,
            };
            let url = link.to_url();
            if let Ok(back) = ChorongLink::parse(&url) {
                assert_eq!(back, link, "url {url:?}");
            }
        }
    }
});
