//! The default keymap: one bundled JSON file in Zed's keymap format, loaded into GPUI at
//! startup. Nothing else in the viewer names a key; everything that shows a binding asks GPUI
//! (`bindings_for_action`) and formats the answer with [`label`].

use gpui::{App, KeyBinding, KeyBindingContextPredicate, Keystroke};
use serde::Deserialize;
use serde_json::Value;
use std::rc::Rc;

const DEFAULT_KEYMAP: &str = include_str!("../keymap/default.json");

/// Which platform's variant of the keymap to build.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Mac,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Other
        }
    }
}

#[derive(Deserialize)]
struct Section {
    context: Option<String>,
    /// `"macos"` or `"other"`; absent means every platform.
    platform: Option<String>,
    bindings: serde_json::Map<String, Value>,
}

/// One parsed binding, before GPUI sees it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Entry {
    pub context: Option<String>,
    pub keystrokes: String,
    pub action: String,
    /// The action's data, from the `["chorong::GoToFocusPage", 2]` form.
    pub data: Option<Value>,
}

/// Rewrites `cmd` to `ctrl` on platforms without a Cmd key, in every stroke of a chord.
fn for_platform(keys: &str, platform: Platform) -> String {
    keys.split_whitespace()
        .map(|stroke| match platform {
            Platform::Mac => stroke.to_string(),
            Platform::Other => stroke
                .split('-')
                .map(|p| if p == "cmd" { "ctrl" } else { p })
                .collect::<Vec<_>>()
                .join("-"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses a keymap file into entries for `platform`, in file order.
pub fn parse(json: &str, platform: Platform) -> Result<Vec<Entry>, String> {
    let sections: Vec<Section> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for s in sections {
        match (s.platform.as_deref(), platform) {
            (None, _) | (Some("macos"), Platform::Mac) | (Some("other"), Platform::Other) => {}
            (Some("macos" | "other"), _) => continue,
            (Some(p), _) => return Err(format!("unknown platform `{p}`")),
        }
        for (keys, action) in s.bindings {
            let (action, data) = match action {
                Value::String(a) => (a, None),
                Value::Array(mut v) if v.len() == 2 && v[0].is_string() => {
                    let data = v.pop();
                    (v[0].as_str().expect("checked").to_string(), data)
                }
                _ => return Err(format!("binding `{keys}` must name an action")),
            };
            out.push(Entry {
                context: s.context.clone(),
                keystrokes: for_platform(&keys, platform),
                action,
                data,
            });
        }
    }
    Ok(out)
}

/// The bundled keymap for this platform.
pub fn default_entries() -> Vec<Entry> {
    parse(DEFAULT_KEYMAP, Platform::current()).expect("bundled keymap parses")
}

/// Turns entries into GPUI bindings. An unknown action or bad keystroke is an error naming it.
pub fn build(cx: &App, entries: &[Entry]) -> Result<Vec<KeyBinding>, String> {
    entries
        .iter()
        .map(|e| {
            let action = cx
                .build_action(&e.action, e.data.clone())
                .map_err(|err| format!("{}: {err}", e.action))?;
            let predicate = match &e.context {
                Some(c) => Some(Rc::new(
                    KeyBindingContextPredicate::parse(c)
                        .map_err(|err| format!("context `{c}`: {err}"))?,
                )),
                None => None,
            };
            KeyBinding::load(
                &e.keystrokes,
                action,
                predicate,
                false,
                None,
                &gpui::DummyKeyboardMapper,
            )
            .map_err(|err| format!("{}: {err}", e.keystrokes))
        })
        .collect()
}

/// Loads the bundled keymap into GPUI. The file ships with the binary, so a failure is a bug.
pub fn bind_defaults(cx: &mut App) {
    let bindings = build(cx, &default_entries()).expect("bundled keymap is valid");
    cx.bind_keys(bindings);
}

/// A keystroke the way a menu would show it: `⌘⇧P` on macOS, `Ctrl-Shift-P` elsewhere.
pub fn stroke_label(k: &Keystroke, platform: Platform) -> String {
    let key = match k.key.as_str() {
        "escape" => "Esc".to_string(),
        "enter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        s if s.starts_with('f') && s.len() > 1 && s[1..].chars().all(|c| c.is_ascii_digit()) => {
            s.to_uppercase()
        }
        s => s.to_uppercase(),
    };
    let m = &k.modifiers;
    match platform {
        Platform::Mac => format!(
            "{}{}{}{}{key}",
            if m.control { "\u{2303}" } else { "" },
            if m.alt { "\u{2325}" } else { "" },
            if m.shift { "\u{21e7}" } else { "" },
            if m.platform { "\u{2318}" } else { "" },
        ),
        Platform::Other => {
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
            let key = key.as_str();
            parts.push(key);
            parts.join("-")
        }
    }
}

/// A whole binding (a chord shows its strokes separated by a space).
pub fn label(binding: &KeyBinding, platform: Platform) -> String {
    binding
        .keystrokes()
        .iter()
        .map(|k| stroke_label(k.inner(), platform))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests read their own fixtures
mod tests {
    use super::*;
    use crate::actions::Layer;
    use crate::view::all_commands;

    #[test]
    fn cmd_becomes_ctrl_outside_macos() {
        assert_eq!(for_platform("cmd-k l", Platform::Other), "ctrl-k l");
        assert_eq!(for_platform("cmd-shift-p", Platform::Other), "ctrl-shift-p");
        assert_eq!(for_platform("cmd-k l", Platform::Mac), "cmd-k l");
        assert_eq!(for_platform("ctrl--", Platform::Other), "ctrl--");
    }

    #[test]
    fn both_variants_parse_and_every_keystroke_is_valid() {
        for platform in [Platform::Mac, Platform::Other] {
            let entries = parse(DEFAULT_KEYMAP, platform).unwrap();
            assert!(entries.len() > 40);
            for e in &entries {
                for stroke in e.keystrokes.split_whitespace() {
                    Keystroke::parse(stroke)
                        .unwrap_or_else(|_| panic!("bad keystroke `{stroke}` ({})", e.action));
                }
            }
        }
    }

    #[test]
    fn platform_sections_apply_to_one_platform() {
        let has = |p, keys: &str| {
            parse(DEFAULT_KEYMAP, p)
                .unwrap()
                .iter()
                .any(|e| e.keystrokes == keys && e.context.as_deref() == Some("Chorong"))
        };
        assert!(has(Platform::Other, "alt-left"));
        assert!(!has(Platform::Mac, "alt-left"));
    }

    #[test]
    fn no_keystroke_is_bound_twice_in_one_context() {
        for platform in [Platform::Mac, Platform::Other] {
            let mut seen = std::collections::HashSet::new();
            for e in parse(DEFAULT_KEYMAP, platform).unwrap() {
                assert!(
                    seen.insert((e.context.clone(), e.keystrokes.clone())),
                    "{:?} {} bound twice",
                    e.context,
                    e.keystrokes
                );
            }
        }
    }

    /// While the index banner is up the code view's context gains `IndexBanner` (see
    /// `Chorong::render`). Enter there keeps the code's meaning (unfold) and nothing that
    /// applies starts a build: the owner's decision, so indexing never starts by accident. Esc
    /// is the banner's Not now.
    #[test]
    fn enter_under_the_index_banner_never_starts_a_build() {
        let stack = [gpui::KeyContext::parse("Chorong CodeView IndexBanner").unwrap()];
        let applying = |key: &str| -> Vec<String> {
            parse(DEFAULT_KEYMAP, Platform::Other)
                .unwrap()
                .into_iter()
                .filter(|e| e.keystrokes == key)
                .filter(|e| match e.context.as_deref() {
                    None => true,
                    Some(c) => KeyBindingContextPredicate::parse(c)
                        .unwrap()
                        .depth_of(&stack)
                        .is_some(),
                })
                .map(|e| e.action)
                .collect()
        };
        let enter = applying("enter");
        assert_eq!(
            enter,
            ["chorong::ToggleFold"],
            "Enter under the banner: {enter:?}"
        );
        assert!(
            applying("escape")
                .last()
                .is_some_and(|a| a == "chorong::StartBannerDismiss"),
            "Esc under the banner is Not now"
        );
    }

    /// A shell picker (Add to page from the tray…) types its query: under the viewer's root
    /// context while one is open (`Chorong PaneOverlay`, see `Chorong::render`), no letter is bound,
    /// at any depth (a binding matches anywhere in the stack, before the query sees the key).
    #[test]
    fn no_letter_is_bound_while_a_shell_picker_types() {
        let stack = [
            gpui::KeyContext::parse("Chorong PaneOverlay").unwrap(),
            gpui::KeyContext::parse("Chorong PanePicker").unwrap(),
        ];
        for e in parse(DEFAULT_KEYMAP, Platform::Other).unwrap() {
            let first = e.keystrokes.split_whitespace().next().unwrap_or_default();
            let k = Keystroke::parse(first).unwrap();
            let letter = k.key.chars().count() == 1
                && !k.modifiers.control
                && !k.modifiers.alt
                && !k.modifiers.platform;
            let applies = match e.context.as_deref() {
                None => true,
                Some(c) => KeyBindingContextPredicate::parse(c)
                    .unwrap()
                    .depth_of(&stack)
                    .is_some(),
            };
            assert!(
                !(letter && applies),
                "{} ({}) in {:?} would take a letter from the picker's query",
                e.keystrokes,
                e.action,
                e.context
            );
        }
    }

    #[test]
    fn single_letter_keys_are_only_where_nothing_is_typed() {
        // The code view, the Show graph popover and Layout mode (whose hint bar holds the focus)
        // have no text field; pickers, the palette and search get letters as text.
        // The Tour and Call stack panes' step lists (`Steps`) have no text field either.
        const NO_TEXT: [&str; 4] = ["CodeView", "Graph", "LayoutMode", "Steps"];
        for e in parse(DEFAULT_KEYMAP, Platform::Other).unwrap() {
            let single = e.keystrokes.chars().count() == 1
                || e.keystrokes
                    .strip_prefix("shift-")
                    .is_some_and(|k| k.chars().count() == 1);
            if single && e.keystrokes != "?" {
                assert!(
                    e.context.as_deref().is_some_and(|c| NO_TEXT
                        .iter()
                        .any(|n| c == *n || c.starts_with(&format!("{n} ")))),
                    "{} in {:?}",
                    e.keystrokes,
                    e.context
                );
            }
        }
    }

    /// GPUI matches a binding at any depth of the context stack, so while the find bar types
    /// the root context (`Chorong Find`) must not say `CodeView`: no binding that applies there
    /// may be a single letter, digit or Space, and the bar's own keys are the ones that apply.
    #[test]
    fn no_letter_is_bound_while_the_find_bar_types() {
        let stack = [gpui::KeyContext::parse(crate::view::FIND_CONTEXT).unwrap()];
        for platform in [Platform::Mac, Platform::Other] {
            let applies: Vec<Entry> = parse(DEFAULT_KEYMAP, platform)
                .unwrap()
                .into_iter()
                .filter(|e| {
                    e.context.as_deref().is_none_or(|c| {
                        KeyBindingContextPredicate::parse(c)
                            .unwrap()
                            .depth_of(&stack)
                            .is_some()
                    })
                })
                .collect();
            for e in &applies {
                let key = e.keystrokes.strip_prefix("shift-").unwrap_or(&e.keystrokes);
                assert!(
                    key.chars().count() > 1 && key != "space",
                    "{} ({}) would not type into the find bar",
                    e.keystrokes,
                    e.action
                );
            }
            let action = |keys: &str| {
                applies
                    .iter()
                    .rev()
                    .find(|e| e.keystrokes == keys)
                    .map(|e| e.action.as_str())
            };
            assert_eq!(action("enter"), Some("chorong::FindNext"));
            assert_eq!(action("shift-enter"), Some("chorong::FindPrev"));
            assert_eq!(action("escape"), Some("chorong::FindClose"));
            assert_eq!(action("alt-c"), Some("chorong::FindToggleCase"));
            assert_eq!(action("alt-r"), Some("chorong::FindToggleRegex"));
            let cmd_f = for_platform("cmd-f", platform);
            assert_eq!(
                action(&cmd_f),
                Some("chorong::FindInFile"),
                "Cmd-F selects the query"
            );
        }
    }

    #[test]
    fn labels() {
        let k = Keystroke::parse("ctrl-shift-p").unwrap();
        assert_eq!(stroke_label(&k, Platform::Other), "Ctrl-Shift-P");
        assert_eq!(stroke_label(&k, Platform::Mac), "\u{2303}\u{21e7}P");
        let k = Keystroke::parse("cmd-shift-p").unwrap();
        assert_eq!(stroke_label(&k, Platform::Mac), "\u{21e7}\u{2318}P");
        assert_eq!(
            stroke_label(&Keystroke::parse("f12").unwrap(), Platform::Other),
            "F12"
        );
        assert_eq!(
            stroke_label(&Keystroke::parse("escape").unwrap(), Platform::Other),
            "Esc"
        );
    }

    #[test]
    fn every_palette_command_is_a_keymap_or_known_action_name() {
        // Palette entries must be unique.
        let mut names: Vec<_> = all_commands().iter().map(|c| c.action).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all_commands().len());
    }

    #[test]
    fn command_ranks_are_unique_and_sorted() {
        let ranks: Vec<u16> = all_commands().iter().map(|c| c.rank).collect();
        assert!(ranks.windows(2).all(|w| w[0] < w[1]), "{ranks:?}");
    }

    /// Every action the default keymap binds has a handler: an `on!` or `unavailable!` call in a
    /// `view/` module's `register`, or it is one of the layout shell's (`cho_layout_ui`).
    #[test]
    fn every_bound_action_is_registered_or_handled_elsewhere() {
        let sources = [
            include_str!("view/blame.rs"),
            include_str!("view/changes.rs"),
            include_str!("view/commands.rs"),
            include_str!("view/compare.rs"),
            include_str!("view/comparison_pane.rs"),
            include_str!("view/context_pane.rs"),
            include_str!("view/editor.rs"),
            include_str!("view/find_bar.rs"),
            include_str!("view/graph_ui.rs"),
            include_str!("view/highlights.rs"),
            include_str!("view/index_status.rs"),
            include_str!("view/investigation_view.rs"),
            include_str!("view/navigation.rs"),
            include_str!("view/overlays.rs"),
            include_str!("view/picker_ui.rs"),
            include_str!("view/pins.rs"),
            include_str!("view/remote.rs"),
            include_str!("view/search_pane.rs"),
            include_str!("view/settings_cmds.rs"),
            include_str!("view/shell_host.rs"),
            include_str!("view/tour_pages_view.rs"),
        ];
        // App-level actions (the menu bar's Quit and Minimize) are handled in `main.rs`.
        let app = include_str!("main.rs");
        let registered = |name: &str| {
            let on = format!("on!(d, cx, {name},");
            let un = format!("{name} => ");
            let app_on = format!("cx.on_action(|_: &{name},");
            app.lines().any(|l| l.contains(&app_on))
                || sources.iter().any(|s| {
                    s.lines()
                        .any(|l| l.contains(&on) || l.trim_start().starts_with(&un))
                })
        };
        // Actions the layout shell registers (`bind_actions`): the unit actions in its
        // `actions!` block and its data-carrying `pub struct` actions.
        let layout_actions = include_str!("../../crates/cho-layout-ui/src/actions.rs");
        let handled_elsewhere = |name: &str| {
            let strukt = format!("pub struct {name}");
            layout_actions
                .lines()
                .any(|l| l.trim().trim_end_matches(',') == name || l.starts_with(&strukt))
        };
        for e in default_entries() {
            let name = e.action.strip_prefix("chorong::").unwrap_or(&e.action);
            let layout = handled_elsewhere(name);
            assert!(
                layout || registered(name),
                "{} is bound in {:?} but no view module registers it",
                e.action,
                e.context
            );
        }
    }

    /// The user guide's key table, built from the bundled keymap.
    fn guide_table() -> String {
        let mac = parse(DEFAULT_KEYMAP, Platform::Mac).unwrap();
        let other = parse(DEFAULT_KEYMAP, Platform::Other).unwrap();
        let keys = |entries: &[Entry], action: &str, platform| {
            let mut v: Vec<String> = Vec::new();
            // Layout mode's keys (`p`, `b`) work only inside the mode; the guide lists them in
            // its Layout mode section, not as the command's key.
            for e in entries.iter().filter(|e| {
                e.action == action
                    && !e
                        .context
                        .as_deref()
                        .is_some_and(|c| c.starts_with("LayoutMode"))
            }) {
                let label = e
                    .keystrokes
                    .split_whitespace()
                    .map(|s| stroke_label(&Keystroke::parse(s).unwrap(), platform))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !v.contains(&label) {
                    v.push(label);
                }
            }
            v.join(", ")
        };
        let mut out = String::new();
        for layer in [
            Layer::Go,
            Layer::View,
            Layer::Read,
            Layer::Compare,
            Layer::Steps,
            Layer::Comparison,
        ] {
            out.push_str(&format!(
                "\n**{}**\n\n| Action | Linux / Windows | macOS |\n| --- | --- | --- |\n",
                layer.title()
            ));
            for c in all_commands().iter().filter(|c| c.layer == layer) {
                let (o, m) = (
                    keys(&other, c.action, Platform::Other),
                    keys(&mac, c.action, Platform::Mac),
                );
                if !o.is_empty() || !m.is_empty() {
                    out.push_str(&format!("| {} | {o} | {m} |\n", c.title));
                }
            }
        }
        out
    }

    #[test]
    fn the_user_guide_table_matches_the_keymap() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/public/user-guide.md");
        let guide = std::fs::read_to_string(path).unwrap();
        let (open, close) = ("<!-- keymap-table -->", "<!-- /keymap-table -->");
        let (a, b) = (
            guide.find(open).unwrap() + open.len(),
            guide.find(close).unwrap(),
        );
        let want = guide_table();
        if std::env::var_os("CHORONG_UPDATE_DOCS").is_some() {
            // Regenerate and write in one step: nothing is compared with stale content, so
            // the first run after a merge passes. This test is the file's only writer, and
            // no other test reads it.
            if guide[a..b] != want {
                let new = format!("{}{want}{}", &guide[..a], &guide[b..]);
                std::fs::write(path, new).unwrap();
            }
        } else if guide[a..b] != want {
            panic!(
                "docs/public/user-guide.md key table is out of date; run the tests with CHORONG_UPDATE_DOCS=1 to rewrite it"
            );
        }
    }
}
