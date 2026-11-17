- The investigation skill (`cho skill investigation`) now writes format version 2, and
  `cho formats` reports version 2 (version 1 files still load). It teaches choosing between a
  call stack, a tour (including several files side by side with highlighted chunks), a
  comparison and a diagram; views, highlights, `from`/`to` ranges, external nodes, phases,
  evidence, runtime values, comparison results and branches; and getting declaration names,
  uses and branch changes from Chorong instead of grepping. Every example in it is checked
  against the format by the test suite.
