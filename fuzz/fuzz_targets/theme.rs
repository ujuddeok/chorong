#![no_main]
use libfuzzer_sys::fuzz_target;
use cho_theme::ThemeFamily;

fuzz_target!(|data: &[u8]| {
    // First byte splits the input into a JSON document and capture-name material.
    let Some((&split, rest)) = data.split_first() else { return };
    let cut = (split as usize).min(rest.len());
    let json = String::from_utf8_lossy(&rest[..rest.len() - cut]);
    let names = String::from_utf8_lossy(&rest[rest.len() - cut..]);
    let Ok(family) = ThemeFamily::parse(&json) else { return };
    let mut queries: Vec<String> = names.split_whitespace().map(str::to_owned).collect();
    queries.extend(["", ".", "..", "a.", ".a", "function.method.x", "é.ü.日本"].map(str::to_owned));
    queries.push(names.into_owned());
    for theme in &family.themes {
        for q in &queries {
            let _ = theme.syntax_style(q);
        }
    }
    let _ = family.theme(None);
    for theme in &family.themes {
        let _ = family.theme(Some(&theme.name));
    }
});
