//! The lab's keymap: one JSON file in Zed's keymap format, loaded into GPUI at startup. The UI
//! never names a key; it asks GPUI for an action's binding and formats it with
//! `cho_layout_ui::keys::label`.
//! (The viewer has its own loader in `app/src/keymap.rs`; on integration the lab's bindings
//! move into the viewer's keymap file and this module goes away.)

use gpui::{App, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;
use std::rc::Rc;

const LAB_KEYMAP: &str = include_str!("../keymap/lab.json");
/// Optional: `h/j/k/l` in Layout mode, for Vim users (`--vim`).
const VIM_KEYMAP: &str = include_str!("../keymap/vim.json");

#[derive(Deserialize)]
struct Section {
    context: Option<String>,
    bindings: serde_json::Map<String, serde_json::Value>,
}

/// Rewrites `cmd` to `ctrl` off macOS, in every stroke of a chord.
fn for_platform(keys: &str) -> String {
    keys.split_whitespace()
        .map(|stroke| {
            if cfg!(target_os = "macos") {
                stroke.to_string()
            } else {
                stroke
                    .split('-')
                    .map(|p| if p == "cmd" { "ctrl" } else { p })
                    .collect::<Vec<_>>()
                    .join("-")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One binding of a keymap file, in file order.
pub struct Entry {
    pub context: Option<String>,
    /// Keystrokes, for this platform.
    pub keys: String,
    pub action: String,
    /// The action's data (Zed's `["chorong::GoToFocusPage", 2]` form).
    pub data: Option<serde_json::Value>,
}

/// Parses the bundled keymap.
pub fn entries() -> Result<Vec<Entry>, String> {
    entries_from(LAB_KEYMAP)
}

fn entries_from(json: &str) -> Result<Vec<Entry>, String> {
    let sections: Vec<Section> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for s in sections {
        for (keys, action) in s.bindings {
            let (action, data) = match action {
                serde_json::Value::String(a) => (a, None),
                serde_json::Value::Array(mut v) if v.len() == 2 && v[0].is_string() => {
                    let data = v.pop();
                    (v[0].as_str().expect("checked").to_string(), data)
                }
                _ => return Err(format!("binding `{keys}` must name an action")),
            };
            out.push(Entry {
                context: s.context.clone(),
                keys: for_platform(&keys),
                action,
                data,
            });
        }
    }
    Ok(out)
}

/// Loads the bundled keymap, and the Vim one on top when asked. They ship with the binary, so
/// a failure is a bug.
pub fn bind(cx: &mut App, vim: bool) {
    let mut all = entries();
    if vim && let Ok(list) = &mut all {
        list.extend(entries_from(VIM_KEYMAP).expect("bundled vim keymap parses"));
    }
    let bindings = all
        .and_then(|es| {
            es.iter()
                .map(|e| {
                    let Entry {
                        context,
                        keys,
                        action,
                        data,
                    } = e;
                    let action = cx
                        .build_action(action, data.clone())
                        .map_err(|e| format!("{action}: {e}"))?;
                    let predicate = context
                        .as_deref()
                        .map(KeyBindingContextPredicate::parse)
                        .transpose()
                        .map_err(|e| e.to_string())?
                        .map(Rc::new);
                    KeyBinding::load(
                        keys,
                        action,
                        predicate,
                        false,
                        None,
                        &gpui::DummyKeyboardMapper,
                    )
                    .map_err(|e| format!("{keys}: {e}"))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .expect("bundled lab keymap is valid");
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_keymap_parses_and_names_layout_md_keys() {
        let es = super::entries().unwrap();
        let find = |a: &str| es.iter().find(|e| e.action == a).map(|e| e.keys.clone());
        let page2 = es
            .iter()
            .find(|e| e.action == "chorong::GoToFocusPage" && e.data == Some(serde_json::json!(2)));
        assert!(page2.is_some(), "Cmd-K 2 goes to page 2");
        assert!(super::entries_from(super::VIM_KEYMAP).unwrap().len() >= 12);
        let cmd = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        assert_eq!(
            find("chorong::ToggleMaximizePane"),
            Some(format!("{cmd}-k m"))
        );
        assert_eq!(find("chorong::RestoreLayout"), Some("escape".into()));
        assert_eq!(
            find("chorong::PinContextToBottomBar"),
            Some(format!("{cmd}-k shift-p"))
        );
    }
}
