# The README demo. Regenerate with demo/record.sh.
Output demo/fleetdeck.gif

Require fleetdeck

Set Shell bash
Set FontFamily "JetBrains Mono"
Set FontSize 14
Set Width 1280
Set Height 760
Set Padding 12
Set Framerate 15
Set TypingSpeed 50ms
Set Theme "Catppuccin Mocha"

Hide
Type "export PATH=$PWD/demo/bin:$PATH FLEETDECK_DEMO_ROOT=/home/captain XDG_CONFIG_HOME=/home/captain/.config PS1='$ ' && demo/setup.sh /home/captain && clear"
Enter
Sleep 2s
Show

Type "fleetdeck"
Sleep 400ms
Enter
Sleep 2s

# Walk the homes: the main home, its second mate, the remote home.
Type "g"
Sleep 1.5s
Screenshot demo/work.png
Sleep 1s
Down
Sleep 1.8s
Down
Sleep 2s
Type "g"
Sleep 800ms

# Open a task in the detail view.
Tab
Sleep 600ms
Enter
Sleep 2.5s
Screenshot demo/detail.png
Sleep 1s
Escape
Sleep 400ms
Escape
Sleep 400ms

# PRs, read with the offline gh stand-in.
Type "p"
Sleep 2.5s
Screenshot demo/prs.png
Sleep 1s

Type "3"
Sleep 2.5s
Type "4"
Sleep 2.5s
Type "q"
Sleep 300ms
