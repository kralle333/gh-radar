# Records demo.gif for the README. Run from the repo root:
#   vhs demo/demo.tape
# Uses demo/config.toml, which only tracks public repos.

Output demo/demo.gif

Require gh

Set Shell zsh
Set Theme "Catppuccin Mocha"
Set FontSize 15
Set Width 1500
Set Height 760
Set Padding 16
Set Framerate 24
Set TypingSpeed 70ms

Env GH_RADAR_CONFIG "demo/config.toml"

# Start off-camera so the recording opens on a loaded dashboard.
Hide
Type "gh radar"
Enter
Sleep 9s
Show

Sleep 3s

# Browse repos; the details pane shows the latest run and its jobs.
Down
Sleep 3s
Down
Sleep 3s

# Sort failing and running repos to the top.
Type "s"
Sleep 3s
Down
Sleep 3s

# Filter, then open the repo's runs; the side pane shows the failing jobs.
Type "/tokio"
Sleep 0.8s
Enter
Sleep 0.8s
Enter
Sleep 4.5s
Down
Sleep 4.5s
Escape
Sleep 0.8s
Escape
Sleep 1.5s

# Key overview.
Type "?"
Sleep 3.5s
Escape
Sleep 1s

Type "q"
Sleep 0.5s
