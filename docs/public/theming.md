# Theming

Chorong reads **Zed theme files** (schema v0.2.0) directly, so any Zed theme works.
Code: `crates/cho-theme`.

```bash
chorong-app --theme path/to/theme.json --theme-name "One Dark"
cho theme path/to/theme.json        # validate and list themes
```

The bundled family, **Chorong Dark** and **Chorong Light**, is original to this project
(`crates/cho-theme/themes/chorong.json`). Before bundling any third-party theme or font,
check its licence.

## Keys Chorong uses

`background`, `border`, `text`, `text.muted`, `panel.background`, `status_bar.background`,
`title_bar.background`, `element.selected`, `editor.background`, `editor.foreground`,
`editor.gutter.background`, `editor.line_number`, `editor.active_line_number`,
`editor.active_line.background`, `players` and `syntax`. Missing keys fall back to values
derived from `background` and `text`. Unknown keys are ignored; malformed colours are an error.

## Syntax

Grammars emit nvim-style captures (`keyword.function`, `comment.documentation`,
`variable.member`). `cho-syntax` maps each to a Zed key (`keyword`, `comment.doc`,
`property`), and lookup falls back from a dotted key to its parent (`function.method` →
`function`). The mapping table is `CAPTURES` in `crates/cho-syntax/src/highlight.rs`.

## Players

Zed's `players` (per-collaborator colours) are reused as Chorong's palette for anything
that needs several distinct, theme-matched colours: pinned occurrence groups, caller pills,
diagram groups.
