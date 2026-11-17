- Opening a file with very deep nesting (tens of thousands of brackets, or a very long
  `a + b + …` or `.call()` chain, as in generated code) no longer crashes Chorong or takes minutes:
  past 2,000 levels the file is shown without syntax colours, and declarations inside more than
  64 nested scopes are left out of the outline.
