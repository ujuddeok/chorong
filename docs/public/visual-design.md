# Visual design

How the viewer looks: the file tree, panes and sections, type, spacing, icons, kind marks,
row states, surfaces and the theme keys behind each. Everything here is styling of panes that
exist today. Docks, pinning, tours and focus layouts are planned.

The look follows Zed's own UI (project panel, outline panel, status bar) so that any Zed theme
still works: every colour comes from a theme key, with a derived fallback when a theme leaves
the key out. Nothing is a hard-coded colour except the bundled Chorong themes themselves.

## What must not be lost

The current UI shows these, and the new design keeps every one:

- **Context pane:** the enclosing **Declaration** *and* the **Symbol** under the pointer or
  caret, as separate sections (they differ whenever you point at something inside a function),
  then Definition and References. Without an index, the "Definition and references need an
  index. Run `cho index`." note stays where they would be. It is never replaced by a name
  search.
- **Status bar:** language, line count, `parse complete` / `partial parse`, declaration count,
  `Index: none | loading | current | stale | failed`, `Ln, Col`, the highlight mode, Critical
  Lens with its legend, `Read-only`, Open in (editor), theme name.
- **Outline:** each declaration's kind and line count, indented by scope.
- **Search:** query box with case and regex toggles, the summary line (including the 5,000 cap
  and regex errors), results grouped by file with counts.

## File tree

Replaces the flat file list.

- Folders first, then files, each case-insensitively alphabetical.
- A folder row is chevron (`chevron-right` / `chevron-down`), folder icon (`folder` /
  `folder-open`), name. A file row reserves the chevron slot, so icons line up.
- **Compact folders:** a chain of folders that each hold only one folder shows as one row,
  `fuzz / fuzz_targets`, with the separators in `text.muted` (Zed's `auto_fold_dirs`).
- **Indent:** 12 px per level. Indent guides are 1 px lines in `panel.indent_guide`; the guide
  of the selected file's parent folder uses `panel.indent_guide_active`.
- **Reveal:** opening a file (from anywhere) expands its ancestors and scrolls it into view.
- **Sticky ancestors:** while scrolling, the open file's ancestor folders stay pinned at the top,
  at most three rows.
- **Keyboard:** Up and Down move, Left collapses (or goes to the parent), Right expands, Enter
  opens. Key context `FileTree`.
- **Pane header:** the root folder name, with buttons for search and collapse all.
- The search panel still replaces the tree in the same pane, using the same row styles.

Not built yet: keyboard movement in the tree, sticky ancestors, the header's search and
collapse-all buttons, and the focused-pane inset border on the open file's row.

## Panes and sections

Two levels of heading:

| Level | Height | Text | Separator |
| --- | --- | --- | --- |
| Pane header (Context, Outline, Search, the tree root) | 28 px | 12 px Medium, `text`; counts and status at 11 px `text.muted` on the right | 1 px `border` underneath |
| Section header (Declaration, Symbol, Definition, References) | 22 px | 11 px Semibold, sentence case, `text.muted`, with a collapse chevron (not built yet) | 8 px of space above, no rule |

- `border` separates panes; `border.variant` (fainter) separates things inside a pane, such as
  card edges.
- A section header may carry a short qualifier: `Declaration · you are in` and
  `Symbol · under the pointer` / `at the caret` in the faint `editor.line_number` colour,
  counts such as `References · 14 in 3 files` in `text.muted`.
- The Context pane header says what it follows: an eye icon and `hovering`, or `at caret`.

## Type

UI font: IBM Plex Sans; code: IBM Plex Mono. Both ship with Chorong (SIL Open Font License), so
text looks the same everywhere; `appearance.ui_font` and `appearance.code_font` choose any
installed family instead.

| Role | Size | Weight | Colour |
| --- | --- | --- | --- |
| Pane title | 12 | Medium | `text` |
| Row label: tree names, outline names, context values | 12.5 | Regular; Medium when selected, current or a container (struct, class, impl) | `text` |
| Section header | 11 | Semibold | `text.muted` |
| Meta: paths, counts, line ranges, kind words | 11 | Regular | `text.muted` |
| Code previews: references, definition, search results | 12 mono | Regular | syntax colours |
| Status bar | 11.5 | Regular | `text.muted`; the language and position in `text` |

Muted text is for metadata, never for names. Bold is reserved for matched characters in the
picker and search.

## Spacing and density

A 4 px grid.

| Element | Size |
| --- | --- |
| Pane header | 28 px |
| Tree, outline and search-group rows, section headers | 22 px |
| Reference and search preview rows | 20 px |
| Status bar | 24 px |
| Row inset from the pane edge | 4 px, radius 4 px |
| Pane edge padding | 8–10 px |
| Chevron to icon / icon to label | 4 px / 6 px |
| Tree indent / outline indent | 12 px / 14 px |

One density for now.

## Icons

- **Set:** [Lucide](https://lucide.dev), **ISC licence**. Ship only the SVGs used (about 30)
  under `app/assets/icons/`, with the ISC notice beside them, loaded through GPUI's
  `AssetSource` and drawn with `svg().path(..).text_color(..)`, so every icon is one colour
  and tinted by the theme.
- Do not copy Zed's `assets/icons`: their provenance and licence are mixed, and Zed's code is
  GPL.
- **File icons by type, one colour (`icon`):** `file-code` for source, `file-json` for config
  (`Cargo.toml`, `package.json`, `tsconfig.json`), `file-text` for Markdown and text, `lock`
  for lockfiles, `folder` / `folder-open`. No per-language logos: they clash with themes and
  carry trademark issues.
- UI icons used so far: `search`, `chevrons-down-up` (collapse all), `x`, `eye`, `pin`,
  `lock`, `external-link`, `info`, `triangle-alert`, `loader`.

## Kind marks

The outline, context pane and breadcrumb mark a declaration's kind with a 14 px glyph, tinted
with the syntax colour that kind has in the code. The kind word is still shown next to the
name in the context pane (Declaration and Symbol lines) and as the outline row's tooltip, so
nothing depends on recognising a glyph.

| Kind | Glyph (Lucide) | Colour (syntax key, then fallback) |
| --- | --- | --- |
| Function, Method | `square-function` | `function` |
| Struct, Class | `box` | `type` |
| Interface (trait, protocol) | `component` | `type` |
| Enum | `list` | `enum` → `type` |
| Impl | `layers` | `keyword` |
| Module | `package` | `namespace` → `type` |
| Type alias | `type` | `type` |
| Constant | `hash` | `constant` |
| Field (Java) | `variable` | `property` → `variable` |
| Macro | `zap` | `function.macro` → `function` |

The alternative was a letter badge (`f`, `m`, `S`, `I`, `E`, `i`, `M`, `T`, `c`, `!` in a
tinted rounded square). Glyphs were chosen; keep the mapping in one table so the letter form
can be offered as a setting later.

**Outline:** glyph, name, then the line count right-aligned at 11 px `text.muted`, always
visible. Containers (struct, class, impl) are Medium, and their members are indented under
them with indent guides. The declaration containing the caret gets the *current* marker.

**Context pane:** the Declaration is a card (glyph, `Type::` prefix in `text.muted`, name in
Medium; `kind · lines 168–179` underneath) on `editor.background`-derived card fill with a
`border.variant` edge. Symbol is a single line: glyph, name, kind word. Definition is a file
row (icon, name, folder, `:line`) over the signature in the code font. References are grouped
by file: a file row (chevron, icon, name, folder, count pill) then preview rows indented under
it, `N more in this file` when truncated.

**Breadcrumb** (not built yet)**:** the code pane header shows the path (`crates › cho-core › src › buffer.rs`) and
then the enclosing declarations with their glyphs (`SourceBuffer › utf16_to_byte`), from the
syntax tree.

## Row states

One recipe for every list (tree, outline, references, search results, picker).

| State | Look | Theme key, then fallback |
| --- | --- | --- |
| Hover | fill | `ghost_element.hover` → `element.hover` → `text` at 6% |
| Selected, pane not focused | fill | `element.selected` |
| Selected, pane focused | fill plus a 1 px inset border | `panel.focused_border` → `border.focused` → `players[0].cursor` |
| Pressed | fill | `element.active` |
| Current, not selected (the open file while focus is elsewhere) | 2 px bar at the row's left edge, Medium text | `text.accent` → `players[0].cursor` |
| Current outline entry (the declaration holding the caret) | the selected fill plus the 2 px bar, Medium text; its parent scope's guide in `panel.indent_guide_active` | `element.selected`, `text.accent` |

## Surfaces

| Surface | Key |
| --- | --- |
| Editor | `editor.background` |
| Side panes | `panel.background` |
| Pane headers | `title_bar.background` |
| Status bar | `status_bar.background` |
| Overlays: picker, palette, Cmd-K popup, menus | `elevated_surface.background`, `border`, shadow |
| Editor header (path and breadcrumb) | `toolbar.background` → `editor.background` |

Many themes give panels and the editor the same colour. Separation then comes from `border`
lines and the header bands, never from colour alone. Check every change against at least one
such theme (One Dark) and one light theme.

## Status bar

Left to right: language (with its file icon), line count, parse status, declaration count, the
index dot and text; then a spacer; then `Ln, Col`, the highlight mode, the Critical Lens chip,
`Read-only` with a lock, **Open in (editor)** as a ghost button, and the theme name. Items are
spaced 14 px apart instead of separated by `·`.

Colour appears only where something needs attention:

| State | Look |
| --- | --- |
| `parse complete` | `text.muted` |
| `partial parse` | warning icon and text in `warning` |
| `Index: current` | solid dot in `success` |
| `Index: stale` | dot and text in `warning`; the context pane also shows "Index is stale: results may be out of date." |
| `Index: failed` | dot and text in `error` |
| `Index: none` | hollow dot (outline in `text.muted`) |
| `Index: loading` | dashed hollow dot |
| File loading | `Loading…` with a spinner, other file fields hidden |
| Critical Lens on | a chip tinted with `text.accent` at 16%: `Critical Lens  calls: repo external unknown`, each legend word drawn in its own call style (bright, 70%, underlined) |

## Theme keys

Keys Chorong reads, in addition to those in [theming.md](theming.md). Every one has a fallback, so
a theme that defines none of them still renders correctly.

| Key | Used for | Fallback |
| --- | --- | --- |
| `border.variant` | rules and card edges inside a pane | `border` at 50% over the panel |
| `border.focused` | focused-row border (second choice) | `players[0].cursor` |
| `element.hover`, `ghost_element.hover` | hovered rows and buttons | `text` at 6% |
| `element.active` | pressed rows | `element.selected` |
| `panel.focused_border` | focused-row border | `border.focused` |
| `panel.indent_guide`, `panel.indent_guide_active` | tree and outline guides | `border.variant`, `border` |
| `icon`, `icon.muted` | file and UI icons | `text.muted` |
| `text.accent` | current marker, pinned and focus accents | `players[0].cursor` |
| `elevated_surface.background` | overlays | `panel.background` |
| `toolbar.background` | editor header | `editor.background` |
| `search.match_background` | search result matches | `players[0].cursor` at 28% |
| `success`, `warning`, `error` | index and parse states, verification labels | none: shown without colour, the text still says it |
| `version_control.modified/added/deleted`, then `modified`, `created`, `deleted` | git markers | none: letter badge only |
| `syntax.*` (existing) | kind marks; context-pane previews (definition and references), highlighted as in the code view, with the symbol's own occurrence in its kind colour, Medium | parent key, as for highlighting |

`cho-theme` parses these alongside the existing keys; unknown keys stay ignored and malformed
colours stay an error.

## Light themes

The same rules apply unchanged; only the theme values differ. The design was reviewed in both
Chorong Dark and Chorong Light.
