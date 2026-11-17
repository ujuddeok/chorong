# Skill examples fixture

A small, made-up order service. Every JSON example in the investigation skill
(`crates/cho-cli/assets/investigation-skill.md`) is checked against these files by
`crates/cho-cli/tests/skill.rs`, so the skill can't drift from the format. The code is never
built; it only has to declare what the examples anchor.
