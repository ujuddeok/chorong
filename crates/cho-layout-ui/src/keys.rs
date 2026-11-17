//! Formatting of key bindings for menus and hints. The UI never names a key; it asks GPUI for
//! an action's binding (in the window's keymap, whatever loaded it) and formats it here.

use gpui::{Action, KeyBinding, Keystroke, Window};

fn stroke(k: &Keystroke) -> String {
    let key = match k.key.as_str() {
        "escape" => "Esc".to_string(),
        "enter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        s => s.to_uppercase(),
    };
    let m = &k.modifiers;
    // A bare letter reads as typed (`r`, as in the Layout mode hint bar); with modifiers it is
    // shown capitalised (`Shift-U`, `Ctrl-K M`).
    let key = if !(m.control || m.alt || m.shift || m.platform) && k.key.chars().count() == 1 {
        k.key.clone()
    } else {
        key
    };
    if cfg!(target_os = "macos") {
        format!(
            "{}{}{}{}{key}",
            if m.control { "\u{2303}" } else { "" },
            if m.alt { "\u{2325}" } else { "" },
            if m.shift { "\u{21e7}" } else { "" },
            if m.platform { "\u{2318}" } else { "" },
        )
    } else {
        let mut parts: Vec<&str> = Vec::new();
        if m.control {
            parts.push("Ctrl");
        }
        if m.alt {
            parts.push("Alt");
        }
        if m.shift {
            parts.push("Shift");
        }
        if m.platform {
            parts.push("Super");
        }
        parts.push(&key);
        parts.join("-")
    }
}

/// The current binding for `action` as a menu shows it (`Ctrl-K M`), if it has one.
pub fn label(action: &dyn Action, window: &Window) -> Option<String> {
    window
        .highest_precedence_binding_for_action(action)
        .map(|b| {
            b.keystrokes()
                .iter()
                .map(|k| stroke(k.inner()))
                .collect::<Vec<_>>()
                .join(" ")
        })
}

fn binding_label(b: &KeyBinding) -> String {
    b.keystrokes()
        .iter()
        .map(|k| stroke(k.inner()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The binding for `action` in key context `context` only (`"LayoutMode"`), so the hint bar
/// shows the mode's key rather than a chord bound elsewhere.
pub fn label_in(action: &dyn Action, window: &Window, context: &str) -> Option<String> {
    let ctx = gpui::KeyContext::parse(context).ok()?;
    window
        .highest_precedence_binding_for_action_in_context(action, ctx)
        .map(|b| binding_label(&b))
}

/// Four arrow bindings shown as one: `Shift-←↑↓→` when they share their modifiers.
pub fn arrows(labels: [Option<String>; 4]) -> Option<String> {
    let labels: Vec<String> = labels.into_iter().collect::<Option<_>>()?;
    let split = |l: &String| {
        let last = l.chars().last()?;
        "←→↑↓"
            .contains(last)
            .then(|| (l[..l.len() - last.len_utf8()].to_string(), last))
    };
    let parts: Vec<(String, char)> = labels.iter().map(split).collect::<Option<_>>()?;
    if parts.iter().all(|(p, _)| *p == parts[0].0) {
        Some(format!(
            "{}{}",
            parts[0].0,
            parts.iter().map(|(_, c)| *c).collect::<String>()
        ))
    } else {
        Some(labels.join(" "))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn arrows_share_modifiers() {
        assert_eq!(
            super::arrows(["←", "↑", "↓", "→"].map(|s| Some(format!("Shift-{s}")))),
            Some("Shift-←↑↓→".into())
        );
    }
}
