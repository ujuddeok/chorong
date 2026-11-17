use super::*;

fn words<'a>(line: &'a str, spans: &[Range<usize>]) -> Vec<&'a str> {
    spans.iter().map(|r| &line[r.clone()]).collect()
}

fn one(base: &str, head: &str) -> Option<PairedLine> {
    paired_lines(&[base], &[head]).into_iter().next()
}

#[test]
fn tokens_are_words_numbers_and_single_punctuation() {
    let line = r#"    format!("s-{user}", 42) // naïve_x"#;
    let t: Vec<&str> = tokens(line).iter().map(|r| &line[r.clone()]).collect();
    assert_eq!(
        t,
        [
            "format", "!", "(", "\"", "s", "-", "{", "user", "}", "\"", ",", "42", ")", "/", "/",
            "naïve_x"
        ]
    );
    assert!(tokens("   \t").is_empty());
}

#[test]
fn a_clean_edit_marks_the_one_word() {
    let base = r#"    format!("s-{user}")"#;
    let head = r#"    format!("session-{user}")"#;
    let p = one(base, head).unwrap();
    assert_eq!(words(base, &p.base_spans), ["s"]);
    assert_eq!(words(head, &p.head_spans), ["session"]);
    assert!(!p.whitespace_only);
}

#[test]
fn a_comment_that_gained_words_marks_them_as_one_span() {
    let base = "/// Opens a session.";
    let head = "/// Opens a session for `user`.";
    let p = one(base, head).unwrap();
    assert!(p.base_spans.is_empty());
    assert_eq!(words(head, &p.head_spans), ["for `user`"]);
}

#[test]
fn a_heavily_rewritten_line_gets_no_word_spans() {
    // Paired (alike enough) or not, more than half its tokens changed: line tint only.
    let r = paired_lines(
        &["let x = foo(a, b);"],
        &["let total = compute(b, a, scale);"],
    );
    assert!(
        r.iter()
            .all(|p| p.base_spans.is_empty() && p.head_spans.is_empty())
    );
}

#[test]
fn swapped_arguments_mark_the_moved_word() {
    let base = "send(user, msg)";
    let head = "send(msg, user)";
    let p = one(base, head).unwrap();
    // The minimal token diff moves `user` (with its comma) past `msg`.
    assert_eq!(words(base, &p.base_spans), ["user,"]);
    assert_eq!(words(head, &p.head_spans), [", user"]);
}

#[test]
fn whitespace_only_is_paired_without_spans() {
    let p = one("if ok { run() }", "    if ok { run() }").unwrap();
    assert!(p.whitespace_only);
    assert!(p.base_spans.is_empty() && p.head_spans.is_empty());
}

#[test]
fn unrelated_lines_in_one_hunk_are_not_paired_by_position() {
    let base = ["let cfg = load()?;", "validate(&cfg)?;", r#"log("ready");"#];
    let head = ["let cfg = Config::checked()?;"];
    let r = paired_lines(&base, &head);
    // Only the first old line is alike enough to pair (two of its three words are kept);
    // half of the new line's words changed, which is not more than half: words are marked.
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].base, r[0].head), (0, 0));
    assert_eq!(words(base[0], &r[0].base_spans), ["load"]);
    assert_eq!(words(head[0], &r[0].head_spans), ["Config::checked"]);
}

#[test]
fn mostly_changed_words_fall_back_to_the_line_tint() {
    // Paired (two words kept of two and five: 57% alike) but three of five words are new on
    // After.
    let r = paired_lines(&["call(a);"], &["call(a, b, c, d);"]);
    assert_eq!(r.len(), 1);
    assert!(r[0].base_spans.is_empty() && r[0].head_spans.is_empty());
}

#[test]
fn pairing_finds_the_matching_line_not_the_one_at_the_same_position() {
    let base = ["fn open(user: &str) {"];
    let head = [
        "// A new helper.",
        "fn helper() {}",
        "fn open(user: &User) {",
    ];
    let r = paired_lines(&base, &head);
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].base, r[0].head), (0, 2));
    assert_eq!(words(head[2], &r[0].head_spans), ["User"]);
    assert_eq!(words(base[0], &r[0].base_spans), ["str"]);
}

#[test]
fn gaps_of_three_unchanged_tokens_stay_apart() {
    let base = "a b c d e f g h i j";
    let head = "X b c d Y f g h i j";
    let p = one(base, head).unwrap();
    assert_eq!(words(head, &p.head_spans), ["X", "Y"]);
    let head2 = "X b c Y e f g h i j";
    let p = one(base, head2).unwrap();
    assert_eq!(words(head2, &p.head_spans), ["X b c Y"]);
}

#[test]
fn oversized_regions_are_not_paired() {
    let lines: Vec<String> = (0..101).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(paired_lines(&refs, &refs).is_empty());
}

// ---- Properties, over generated lines (a fixed-seed generator: deterministic) ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const VOCAB: &[&str] = &[
    "a", "b", "user", "msg", "x", "42", "(", ")", ",", ";", "=", "fn", "let", "é", "\"", "{",
];

fn line(rng: &mut Rng) -> String {
    let n = rng.below(10);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(VOCAB[rng.below(VOCAB.len())]);
        if rng.below(2) == 0 {
            s.push(' ');
        }
    }
    s
}

/// A version of `base` with a few tokens replaced, inserted or removed.
fn edit(base: &str, rng: &mut Rng) -> String {
    let mut toks: Vec<String> = tokens(base)
        .iter()
        .map(|r| base[r.clone()].to_string())
        .collect();
    for _ in 0..rng.below(3) {
        let at = rng.below(toks.len() + 1);
        match rng.below(3) {
            0 if at < toks.len() => toks[at] = VOCAB[rng.below(VOCAB.len())].into(),
            1 if at < toks.len() => {
                toks.remove(at);
            }
            _ => toks.insert(at, VOCAB[rng.below(VOCAB.len())].into()),
        }
    }
    toks.join(" ")
}

/// The longest common subsequence length, by plain dynamic programming.
fn lcs(a: &[u32], b: &[u32]) -> usize {
    let mut t = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            t[i][j] = if a[i] == b[j] {
                1 + t[i + 1][j + 1]
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    t[0][0]
}

#[test]
fn the_token_diff_is_minimal() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..500 {
        let (a, b) = (line(&mut rng), line(&mut rng));
        let mut ids = HashMap::new();
        let (ta, tb) = (tokenise(&a, &mut ids), tokenise(&b, &mut ids));
        let kept = common(&ta.ids, &tb.ids).unwrap();
        assert_eq!(
            kept.iter().filter(|k| **k).count(),
            lcs(&ta.ids, &tb.ids),
            "{a:?} / {b:?}"
        );
    }
}

#[test]
fn every_pair_follows_the_rules() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for _ in 0..300 {
        let base: Vec<String> = (0..1 + rng.below(4)).map(|_| line(&mut rng)).collect();
        let mut head = Vec::new();
        for l in &base {
            match rng.below(4) {
                0 => {}
                1 => head.push(line(&mut rng)),
                _ => head.push(edit(l, &mut rng)),
            }
        }
        let (b, h): (Vec<&str>, Vec<&str>) = (
            base.iter().map(String::as_str).collect(),
            head.iter().map(String::as_str).collect(),
        );
        let pairs = paired_lines(&b, &h);
        assert_eq!(pairs, paired_lines(&b, &h), "deterministic");
        let mut last: Option<(usize, usize)> = None;
        for p in &pairs {
            // In order, one line at most once.
            if let Some((lb, lh)) = last {
                assert!(p.base > lb && p.head > lh);
            }
            last = Some((p.base, p.head));
            let mut ids = HashMap::new();
            let (tb, th) = (tokenise(b[p.base], &mut ids), tokenise(h[p.head], &mut ids));
            assert!(likeness(&tb, &th) >= MIN_SIMILARITY);
            assert_eq!(p.whitespace_only, tb.ids == th.ids);
            for (text, spans) in [(b[p.base], &p.base_spans), (h[p.head], &p.head_spans)] {
                let mut end = 0;
                for r in spans.iter() {
                    assert!(r.start >= end && r.start < r.end && r.end <= text.len());
                    assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
                    // A span starts and ends on a token, never on whitespace.
                    assert!(!text[r.clone()].starts_with(char::is_whitespace));
                    assert!(!text[r.clone()].ends_with(char::is_whitespace));
                    end = r.end;
                }
            }
            if !p.base_spans.is_empty() || !p.head_spans.is_empty() {
                // At most half of either side's counted tokens changed.
                let kept = common(&tb.ids, &th.ids).unwrap();
                let all = |t: &Tokenised| counted(t, &vec![true; t.ids.len()]);
                let same = counted(&tb, &kept);
                assert!(100 * (all(&tb) - same) <= MAX_CHANGED * all(&tb));
                assert!(100 * (all(&th) - same) <= MAX_CHANGED * all(&th));
            }
        }
    }
}
