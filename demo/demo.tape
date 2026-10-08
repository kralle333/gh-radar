# Records demo.gif for the README. Run from the repo root after a release build:
#   cargo build --release && vhs demo/demo.tape
# GH_RADAR_DEMO=1 makes gh radar use fictional accounts and repos instead of
# calling GitHub, so the recording never shows real data.

Output demo/demo.gif

Set Shell zsh
Set Theme "Catppuccin Mocha"
Set FontSize 15
Set Width 1500
Set Height 760
Set Padding 16
Set Framerate 24
Set TypingSpeed 90ms

Env GH_RADAR_DEMO "1"

# Start off-camera so the recording opens on a loaded dashboard.
Hide
Type "./target/release/gh-radar"
Enter
Sleep 2s
Show

Sleep 3s

# Browse your repos; the details pane shows the run and its jobs,
# including which step failed.
Down
Sleep 2.5s
Down
Sleep 3.5s

# Add repos from your personal account and your organizations.
Type "+"
Sleep 2.5s
Tab
Sleep 1.5s
Type "auth"
Sleep 0.8s
Space
Sleep 1.2s
Escape
Sleep 0.6s
Tab
Sleep 1.5s
Type "inv"
Sleep 0.8s
Space
Sleep 1.5s
Enter
Sleep 3s

# Sort failing and running repos to the top, then open one.
Type "s"
Sleep 1.5s
Type "g"
Sleep 2s
Enter
Sleep 3s
Down
Sleep 2.5s
Escape
Sleep 1.5s

Type "q"
Sleep 0.5s
