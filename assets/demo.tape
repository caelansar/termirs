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

Type "s"
Type "./Cargo.toml"
Sleep 200ms
Down 1
Type "/home/devuser/"
Sleep 200ms
Enter
Sleep 1s
Escape

Sleep 500ms

Enter
Type "ls"
Enter
Sleep 1s
Type "rm Cargo.toml"
Enter
Sleep 1s
Escape

Type "d"
Sleep 1s
Type "y"
Sleep 1s
Escape