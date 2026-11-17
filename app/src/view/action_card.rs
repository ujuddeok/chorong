//! Action steps (investigation format v2): a tour step that is an instruction, not code. Its
//! card shows the author's note (inert Markdown) and each command in a monospace block with a
//! **Copy** button. Nothing here runs anything: investigation files are untrusted, so a
//! command is only ever text the reader copies (design/investigations-viewer.md, Security).
//!
//! The same card is the editor's body while an action stop is current in the normal layout
//! (it gives way to the file on Back, a jump or the next stop, like an external node's card)
//! and the main pane of the step's focus page.

use super::*;
use crate::tour_pages::ActionCard;

/// How long a Copy button reads "Copied".
const COPIED_TIME: Duration = Duration::from_millis(1500);

/// Which Copy button was clicked last, and when it stops reading "Copied".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Copied {
    pub key: String,
    pub generation: Generation,
}

impl Chorong {
    /// Puts exactly `text` on the clipboard and shows "Copied" on the button (`key`) and in
    /// the status bar, briefly.
    pub(super) fn copy_command(&mut self, key: String, text: String, cx: &mut Context<Self>) {
        if crate::devlog::on() {
            eprintln!(
                "INV copied {}",
                serde_json::to_string(&text).unwrap_or_default()
            );
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        let mut generation = self
            .copied
            .as_ref()
            .map_or(Generation::default(), |c| c.generation);
        generation.next();
        self.copied = Some(Copied { key, generation });
        self.notify_text("Copied", cx);
        self.dirty_all = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_TIME).await;
            this.update(cx, |this, cx| {
                if this
                    .copied
                    .as_ref()
                    .is_some_and(|c| c.generation.is(generation))
                {
                    this.copied = None;
                    this.dirty_all = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The card for an action step. `heading` is the step line (`Run the migration · step
    /// 2 of 3`); `scope` keeps the buttons' ids apart between panes.
    pub(super) fn render_action_card(
        &self,
        card: &Arc<ActionCard>,
        number: &str,
        heading: String,
        scope: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let note = self.render_note(
            &crate::investigation::note_blocks(&card.note),
            ("action-note", scope),
            cx,
        );
        let commands = card.commands.iter().enumerate().map(|(i, cmd)| {
            let key = format!("{scope}:{number}:{i}");
            let copied = self.copied.as_ref().is_some_and(|c| c.key == key);
            let text = cmd.clipboard().to_string();
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .children(
                    cmd.label
                        .clone()
                        .map(|l| div().text_size(px(11.5)).text_color(c.text_muted).child(l)),
                )
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .p_2()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(c.border_variant)
                        .bg(c.element_bg)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(self.font.clone())
                                .text_size(px(12.5))
                                .whitespace_normal()
                                .children(cmd.display_lines().into_iter().map(|l| {
                                    // An empty line keeps its height.
                                    div().child(if l.is_empty() {
                                        "\u{a0}".to_string()
                                    } else {
                                        l.to_string()
                                    })
                                })),
                        )
                        .child(
                            div()
                                .id(("copy-command", scope * 1000 + i))
                                .flex_none()
                                .px_2()
                                .rounded_sm()
                                .border_1()
                                .border_color(c.border)
                                .bg(c.panel)
                                .text_size(px(12.))
                                .cursor_pointer()
                                .hover(|s| s.bg(c.hover))
                                .when(copied, |d| d.text_color(c.text_accent))
                                .child(if copied { "Copied" } else { "Copy" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.copy_command(key.clone(), text.clone(), cx);
                                })),
                        ),
                )
        });
        div()
            .size_full()
            .child(
                card_shell(620., 8.)
                    .id(("action-card", scope))
                    .border_1()
                    .border_color(c.border)
                    .bg(c.panel)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(action_glyph(self.font.clone(), &c))
                            .child(heading),
                    )
                    .child(
                        div()
                            .italic()
                            .text_size(px(11.5))
                            .text_color(c.text_muted)
                            .child("A step to do by hand: copy each command and run it yourself."),
                    )
                    .child(note)
                    .children(commands),
            )
            .into_any_element()
    }
}

/// The mark an action step shows in lists in place of a location: a shell prompt.
pub(crate) fn action_glyph(
    font: SharedString,
    c: &cho_layout_ui::colors::Colors,
) -> gpui::AnyElement {
    div()
        .flex_none()
        .px(px(3.))
        .rounded(px(3.))
        .border_1()
        .border_color(c.border)
        .font_family(font)
        .text_size(px(10.))
        .text_color(c.text_accent)
        .child("$_")
        .into_any_element()
}

/// The shell of a card in a step pane (an action, a missing anchor, an external system):
/// inset, padded, rounded, a column of `gap`-spaced parts at most `max_w` wide.
pub(crate) fn card_shell(max_w: f32, gap: f32) -> Div {
    div()
        .m_3()
        .p_3()
        .max_w(px(max_w))
        .flex()
        .flex_col()
        .gap(px(gap))
        .rounded(px(6.))
        .text_size(px(12.5))
}
