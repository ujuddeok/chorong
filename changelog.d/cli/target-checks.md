- `cho open` refuses targets with control characters (also when percent-encoded in a URL), clone
  URLs whose bracketed host starts with `-`, and branch names with a part ending in `.lock`, as git
  does, instead of failing later or passing them on.
