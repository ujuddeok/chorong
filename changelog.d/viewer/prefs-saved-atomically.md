- The graph popover's choices (`graph.json`) and Open Remote's last project and "Prepare and
  index" choices (`remote.json`) are saved atomically, so a crash mid-save no longer loses
  them, and a save that fails is reported on the terminal instead of ignored.
