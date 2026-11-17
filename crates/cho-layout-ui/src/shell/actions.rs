//! Binding the layout actions to the shell.

use super::*;

/// Attaches the shell's action handlers to the host's root element, which tracks the host's
/// focus and carries the host's key context, so the actions reach the shell from anywhere
/// under it. `RestoreLayout` propagates when it has nothing to do (no menu, no history list,
/// not in focus), so the host's own Esc handling still runs.
pub fn bind_actions<E: InteractiveElement>(el: E, shell: &Entity<Shell>) -> E {
    Shell::register_actions(el, shell)
}

/// Wraps a handler on the shell as a GPUI action listener that updates the entity.
pub(super) fn listener<A: Action>(
    shell: &Entity<Shell>,
    f: impl Fn(&mut Shell, &A, &mut Window, &mut Context<Shell>) + 'static,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let shell = shell.clone();
    move |a: &A, window: &mut Window, cx: &mut App| {
        shell.update(cx, |s, cx| f(s, a, window, cx));
    }
}

impl Shell {
    // ----- actions ---------------------------------------------------------------------------

    /// Attaches every layout action's handler to `d`.
    pub(super) fn register_actions<E: InteractiveElement>(d: E, shell: &Entity<Shell>) -> E {
        pickers::register(d, shell)
            .on_action(listener(
                shell,
                |this, _: &ToggleMaximizePane, window, cx| {
                    this.close_popup(window, cx);
                    // In a focus layout this restores, even on an empty page (no pane to target).
                    if this.ws.in_focus() {
                        this.apply(cx, |ws| ws.restore(now()).map(|_| ()));
                    } else {
                        this.on_pane(cx, |ws, p| ws.toggle_maximize(p, now()).map(|_| ()));
                    }
                },
            ))
            .on_action(listener(shell, |this, _: &RestoreLayout, window, cx| {
                // Esc: close what is open first; restore only when nothing else is left to do.
                if this.close_popup(window, cx) {
                    return;
                }
                if this.ws.in_focus() {
                    this.apply(cx, |ws| ws.restore(now()).map(|_| ()));
                } else {
                    cx.propagate();
                }
            }))
            .on_action(listener(shell, |this, _: &MinimizePane, _, cx| {
                this.on_pane(cx, |ws, p| ws.toggle_minimize(p).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &PinContextHere, _, cx| {
                let Some(pin) = this.next_pin(cx) else {
                    return;
                };
                this.apply(cx, |ws| ws.pin_context(pin, PinPlace::Here).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &PinContextToBottomBar, _, cx| {
                let Some(pin) = this.next_pin(cx) else {
                    return;
                };
                this.apply(cx, |ws| {
                    ws.pin_context(pin, PinPlace::BottomBar).map(|_| ())
                });
            }))
            .on_action(listener(
                shell,
                |this, _: &ReopenFocusLayout, window, cx| {
                    if matches!(this.popup, Popup::History) {
                        this.close_popup(window, cx);
                    } else {
                        this.open_popup(Popup::History, window, cx);
                    }
                },
            ))
            .on_action(listener(shell, |this, _: &ToggleLeftDock, _, cx| {
                this.apply(cx, |ws| ws.toggle_dock(DockId::Left).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &ToggleRightDock, _, cx| {
                this.apply(cx, |ws| ws.toggle_dock(DockId::Right).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &ToggleBottomBar, _, cx| {
                this.apply(cx, |ws| ws.toggle_dock(DockId::Bottom).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &MovePaneToLeftDock, _, cx| {
                this.on_pane(cx, |ws, p| ws.move_pane(p, Target::Dock(DockId::Left)));
            }))
            .on_action(listener(shell, |this, _: &MovePaneToRightDock, _, cx| {
                this.on_pane(cx, |ws, p| ws.move_pane(p, Target::Dock(DockId::Right)));
            }))
            .on_action(listener(shell, |this, _: &MovePaneToBottomBar, _, cx| {
                this.on_pane(cx, |ws, p| ws.move_pane(p, Target::Dock(DockId::Bottom)));
            }))
            .on_action(listener(shell, |this, _: &SplitPaneRight, _, cx| {
                this.on_pane(cx, |ws, p| ws.split(p, Edge::Right).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &SplitPaneDown, _, cx| {
                this.on_pane(cx, |ws, p| ws.split(p, Edge::Bottom).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &SendPaneToTray, _, cx| {
                this.on_pane(cx, |ws, p| ws.move_pane(p, Target::Tray));
            }))
            .on_action(listener(shell, |this, _: &ClosePane, _, cx| {
                this.on_pane(cx, |ws, p| ws.dismiss(p));
            }))
            .on_action(listener(shell, |this, _: &MovePaneToEditorArea, _, cx| {
                this.on_pane(cx, |ws, p| ws.move_pane(p, Target::EditorArea));
            }))
            .on_action(listener(shell, |this, _: &NextFocusPage, _, cx| {
                this.apply(cx, |ws| ws.next_page());
            }))
            .on_action(listener(shell, |this, _: &PrevFocusPage, _, cx| {
                this.apply(cx, |ws| ws.prev_page());
            }))
            .on_action(listener(shell, |this, _: &NewFocusPage, window, cx| {
                // Outside a focus layout this starts one, with the focused pane; inside it adds
                // a page. Then the tray picker opens (and stays open) to fill the page.
                if this.ws.in_focus() {
                    this.apply(cx, |ws| ws.new_page(None).map(|_| ()));
                } else if let Some(pane) = this.target_pane() {
                    this.apply(cx, |ws| ws.maximize(pane, now()));
                }
                // An empty tray goes straight to the host's file picker.
                if this.ws.in_focus() {
                    this.open_list(super::pickers::ListKind::Bring, window, cx);
                }
            }))
            .on_action(listener(shell, |this, a: &GoToFocusPage, _, cx| {
                // Pages are numbered from 1 in the keymap and the focus bar.
                let index = a.0.saturating_sub(1);
                this.apply(cx, |ws| ws.go_to_page(index));
            }))
            .on_action(listener(shell, |this, _: &CloseFocusPage, _, cx| {
                let current = this.ws.focus().map(|f| f.current);
                if let Some(i) = current {
                    this.apply(cx, |ws| ws.close_page(i, now()));
                }
            }))
            .on_action(listener(
                shell,
                |this, _: &MovePaneToNextFocusPage, _, cx| {
                    if this.ws.in_focus() {
                        this.on_pane(cx, |ws, p| ws.move_pane_to_next_page(p).map(|_| ()));
                    }
                },
            ))
            .on_action(listener(shell, |this, _: &EnterLayoutMode, window, cx| {
                if this.mode.is_some() {
                    this.exit_mode(window, cx);
                } else {
                    this.enter_mode(window, cx);
                }
            }))
            .on_action(listener(shell, |this, _: &ExitLayoutMode, window, cx| {
                this.exit_mode(window, cx);
            }))
            .on_action(listener(shell, |this, _: &EnterResizeMode, _, cx| {
                if let Some(m) = &mut this.mode {
                    m.resize = true;
                    cx.notify();
                }
            }))
            .on_action(listener(shell, |this, _: &ExitResizeMode, _, cx| {
                if let Some(m) = &mut this.mode {
                    m.resize = false;
                }
                // The whole resize session is one undo step.
                this.ws.seal_undo();
                cx.notify();
            }))
            .on_action(listener(shell, |this, _: &FocusPaneLeft, _, cx| {
                this.focus_dir(Direction::Left, cx)
            }))
            .on_action(listener(shell, |this, _: &FocusPaneRight, _, cx| {
                this.focus_dir(Direction::Right, cx)
            }))
            .on_action(listener(shell, |this, _: &FocusPaneUp, _, cx| {
                this.focus_dir(Direction::Up, cx)
            }))
            .on_action(listener(shell, |this, _: &FocusPaneDown, _, cx| {
                this.focus_dir(Direction::Down, cx)
            }))
            .on_action(listener(shell, |this, _: &MovePaneLeft, _, cx| {
                this.move_dir(false, Direction::Left, cx)
            }))
            .on_action(listener(shell, |this, _: &MovePaneRight, _, cx| {
                this.move_dir(false, Direction::Right, cx)
            }))
            .on_action(listener(shell, |this, _: &MovePaneUp, _, cx| {
                this.move_dir(false, Direction::Up, cx)
            }))
            .on_action(listener(shell, |this, _: &MovePaneDown, _, cx| {
                this.move_dir(false, Direction::Down, cx)
            }))
            .on_action(listener(shell, |this, _: &MoveGroupLeft, _, cx| {
                this.move_dir(true, Direction::Left, cx)
            }))
            .on_action(listener(shell, |this, _: &MoveGroupRight, _, cx| {
                this.move_dir(true, Direction::Right, cx)
            }))
            .on_action(listener(shell, |this, _: &MoveGroupUp, _, cx| {
                this.move_dir(true, Direction::Up, cx)
            }))
            .on_action(listener(shell, |this, _: &MoveGroupDown, _, cx| {
                this.move_dir(true, Direction::Down, cx)
            }))
            .on_action(listener(shell, |this, _: &NextTrayFile, _, cx| {
                this.flip_tray(true, cx)
            }))
            .on_action(listener(shell, |this, _: &PrevTrayFile, _, cx| {
                this.flip_tray(false, cx)
            }))
            .on_action(listener(shell, |this, a: &FocusEditorGroup, window, cx| {
                let n = a.0;
                // In Layout mode, a number past the code panes on screen names a tray file.
                let on_screen = this
                    .mode
                    .as_ref()
                    .is_some_and(|m| m.labels.iter().any(|(l, _)| l.to_string() == n.to_string()));
                if this.mode.is_some() && !on_screen && this.bring_tray_label(&n.to_string(), cx) {
                    return;
                }
                this.apply(cx, |ws| ws.focus_editor_group(n).map(|_| ()));
                if this.mode.is_some() {
                    this.exit_mode(window, cx);
                }
            }))
            .on_action(listener(shell, |this, a: &FocusLabel, window, cx| {
                let group = this.mode.as_ref().and_then(|m| {
                    m.labels
                        .iter()
                        .find(|(l, _)| l.to_string() == a.0)
                        .map(|(_, g)| *g)
                });
                if let Some(g) = group {
                    this.apply(cx, |ws| ws.focus_group(g).map(|_| ()));
                    this.exit_mode(window, cx);
                } else {
                    this.bring_tray_label(&a.0, cx);
                }
            }))
            .on_action(listener(shell, |this, a: &ResizePane, _, cx| {
                let dir: Direction = a.direction.into();
                let step = if a.fine {
                    ResizeStep::FINE
                } else {
                    ResizeStep::NORMAL
                };
                this.on_pane(cx, |ws, p| ws.resize_pane(p, dir, step).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &EqualizePanes, _, cx| {
                this.on_pane(cx, |ws, p| ws.equalize_around(p));
            }))
            .on_action(listener(shell, |this, _: &UndoLayout, _, cx| {
                this.apply(cx, |ws| ws.undo())
            }))
            .on_action(listener(shell, |this, _: &RedoLayout, _, cx| {
                this.apply(cx, |ws| ws.redo())
            }))
            .on_action(listener(shell, |this, _: &ToggleAllDocks, _, cx| {
                this.apply(cx, |ws| ws.toggle_all_docks().map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &SplitPaneLeft, _, cx| {
                this.on_pane(cx, |ws, p| ws.split(p, Edge::Left).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &SplitPaneUp, _, cx| {
                this.on_pane(cx, |ws, p| ws.split(p, Edge::Top).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &GrowDock, _, cx| {
                this.apply(cx, |ws| ws.step_focused_dock(true).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &ShrinkDock, _, cx| {
                this.apply(cx, |ws| ws.step_focused_dock(false).map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &ResetDockSize, _, cx| {
                this.apply(cx, |ws| ws.reset_focused_dock().map(|_| ()));
            }))
            .on_action(listener(shell, |this, _: &ToggleFrameStats, _, cx| {
                this.show_stats = !this.show_stats;
                cx.notify();
            }))
    }

    /// Focuses `pane` (so actions act on it) and dispatches `action`, the way the menu and the
    /// header buttons do: every mouse action goes through a named action.
    pub(super) fn dispatch_on(
        &mut self,
        pane: PaneId,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_popup(window, cx);
        let _ = self.ws.activate(pane);
        window.focus(&self.return_focus);
        window.dispatch_action(action, cx);
        cx.notify();
    }
}
