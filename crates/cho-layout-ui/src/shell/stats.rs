//! Frame timing: the input probe, the end-of-frame marker and the frame-time overlay.

use super::*;

impl Shell {
    /// Times every pointer move's dispatch: this listener is registered first (it paints
    /// first), so it runs first in the capture phase and last in the bubble phase.
    pub(super) fn input_probe(&self) -> AnyElement {
        let stats = self.stats.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let stats = stats.clone();
                window.on_mouse_event(move |_: &gpui::MouseMoveEvent, phase, _, _| match phase {
                    gpui::DispatchPhase::Capture => stats.borrow_mut().input_begin(),
                    gpui::DispatchPhase::Bubble => stats.borrow_mut().input_end(),
                });
            },
        )
        .absolute()
        .size_0()
        .into_any_element()
    }

    /// Ends the frame's timing once everything else has painted: deferred elements paint after
    /// the main tree, in priority order, and this one has the highest priority.
    pub(super) fn frame_end(&self) -> AnyElement {
        let stats = self.stats.clone();
        deferred(
            canvas(|_, _, _| {}, move |_, _, _, _| stats.borrow_mut().end())
                .absolute()
                .size_0(),
        )
        .priority(usize::MAX)
        .into_any_element()
    }

    /// The frame-time overlay (chorong::ToggleFrameStats).
    pub(super) fn render_stats(&self, window: &Window) -> AnyElement {
        let c = &self.c;
        let st = self.stats.borrow();
        let s = st.summary();
        let key = keys::label(&ToggleFrameStats, window).unwrap_or_default();
        let line = |label: &str, v: String, warn: bool| {
            div()
                .flex()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(120.))
                        .text_color(c.text_muted)
                        .child(SharedString::from(label.to_string())),
                )
                .child(
                    div()
                        .text_color(if warn {
                            c.warning.unwrap_or(c.text_accent)
                        } else {
                            c.text
                        })
                        .child(SharedString::from(v)),
                )
        };
        let ms = |v: f32| format!("{v:.2} ms");
        deferred(
            div()
                .absolute()
                .top(px(34.))
                .right(px(10.))
                .w(px(250.))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .bg(c.elevated)
                .border_1()
                .border_color(c.border)
                .rounded(px(6.))
                .shadow_lg()
                .font_family(c.mono_font.clone())
                .text_size(px(11.))
                .child(
                    div()
                        .flex()
                        .mb(px(2.))
                        .text_color(c.text_muted)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(div().flex_1().child("Frame time, UI thread"))
                        .child(SharedString::from(key)),
                )
                .child(line("last", ms(s.last), s.last > perf::BUDGET_120))
                .child(line("p50", ms(s.p50), s.p50 > perf::BUDGET_120))
                .child(line("p95", ms(s.p95), s.p95 > perf::BUDGET_120))
                .child(line("p99", ms(s.p99), s.p99 > perf::BUDGET_120))
                .child(line("worst", ms(s.worst), s.worst > perf::BUDGET_120))
                .child(line(
                    "over 8.3 (120 Hz)",
                    format!("{} of {}", s.over_120, s.frames),
                    s.over_120 > 0,
                ))
                .child(line(
                    "over 16.7 (60 Hz)",
                    format!("{} of {}", s.over_60, s.frames),
                    s.over_60 > 0,
                ))
                .child(line(
                    "pointer dispatch",
                    format!("{:.0} µs worst", st.worst_input()),
                    st.worst_input() > perf::BUDGET_120 * 1000.0 / 4.0,
                ))
                .child(line(
                    "last layout op",
                    format!("{:.1} µs", st.last_op_us),
                    false,
                )),
        )
        .priority(1)
        .into_any_element()
    }
}
