- Chorong reads only regular files up to 16 MB from a repository, and refuses a symbolic link that
  leads outside the opened folder. A link, index or symlink naming a device, a pipe, a huge
  file or a file such as `~/.ssh/id_ed25519` now shows a message instead of hanging the viewer
  or displaying it. The CLI, graphs and investigation checks follow the same rule.
