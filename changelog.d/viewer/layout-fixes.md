- Dropping a whole tab group on a focus page chip, and Move pane to next focus page, are one
  undo step each: one Undo puts everything back (it used to take one Undo per tab).
- Move pane to next focus page no longer reports "that would change nothing" after it moved the
  pane to a new page.
- Closing a focus page (or resetting a tour page) forgets the editor read on it, as sending it
  to the tray does, so Restore keeps the normal layout's own active tab.
- The pane menu, the Reopen focus layout list and the pane pickers are never open together:
  opening one closes the others.
- A layout that cannot be saved (a full disk, a read-only state folder) says so once in the
  status bar instead of failing silently.
- The pane pickers (Go to pane…, Move pane to…) edit their query like the file picker: Cmd-A,
  paste, Cmd-Backspace and word delete.
