- The prepare approval prompt shows recipe arguments that contain control characters (terminal
  escapes, newlines) or right-to-left overrides as escaped `$'…'` text, so a repository's
  `.chorong/prepare.json` can't make the prompt show a different command than the one that runs. A
  step folder with such characters is refused.
