Output assets/demo.gif

Set Width 2000
Set Height 1200
Set Theme "Catppuccin Mocha"

Type "clear"
Enter
Type "./termirs"
Sleep 500ms
Enter
Sleep 1s

# create new connection
Type "n"
Type "host.docker.internal"
Sleep 200ms
Down 1
Type "2222"
Sleep 200ms
Down 1
Type "devuser"
Sleep 200ms
Down 1
Type "password"
Sleep 200ms
Down 2
Type "cae"
Sleep 500ms
Enter

Type "ls"
Enter
Sleep 1s
Escape
Sleep 1s

# file explorer
Type "i"
Sleep 1s
Type "/"
Sleep 1s
Type "cargo"
Sleep 1s
Enter
Type "c"
Sleep 1s
Type "j"
Sleep 1s
Type "c"
Sleep 1s
Tab
Type "v"
Sleep 1s
Enter
Sleep 1s
Type 'q'

Sleep 500ms

Enter
Type "ls"
Enter
Sleep 1s
Type "rm Cargo.*"
Enter
Sleep 1s
Escape

Type "d"
Sleep 1s
Type "y"
Sleep 1s
Type 'q'