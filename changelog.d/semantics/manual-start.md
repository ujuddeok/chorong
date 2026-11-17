- Nothing indexes or prepares until you click, even in a folder you trust: files read and
  highlight at once. A banner (a card at the bottom right) says what a build runs, whether the
  project's own code runs and how long the last build took, with **Build index**; an untrusted
  folder's click asks for trust first. A stale index offers **Refresh** and **Keep fresh this
  session**. Remote projects get **Prepare and index** and **Index only**, with the recipe's
  commands; an unapproved one shows the approval panel first. Lightweight mode still shows the
  banner and starts nothing by itself.
- A local TypeScript project without `node_modules` is now indexed, with a warning that imports
  between packages won't resolve until you install the dependencies.
