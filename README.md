# gtk-remote

Drive an existing GTK 3 or GTK 4 app over a local socket. No app changes.

```bash
make
gtk-remote-launch gnome-text-editor
gtk-remote tree --depth 3
gtk-remote press --name Save
```

`gtk-remote-launch` preloads `libgtk_remote.so`, turns the accessibility bridge on, and opens `$XDG_RUNTIME_DIR/gtk-remote/<pid>.sock`. The client talks newline-delimited JSON. Pass `--sock` or `GTK_REMOTE_SOCK` to pick a socket; otherwise it uses the newest one. `GTK_REMOTE=0` disables the preload.

```bash
gtk-remote apps
gtk-remote find --role push-button --name Save
gtk-remote focus --role entry --placeholder "Search"
gtk-remote type --text "hello"
gtk-remote set-text --name Name --text "Ada"
gtk-remote click-at --name Canvas --x 10 --y 10
gtk-remote do --name Menu --action show-menu
```

Selectors: `--id`, `--name`, `--role`, `--placeholder`, `--description`, `--class`, `--index`. Add `--pretty` for indented JSON.

LibreOffice Calc's grid is not a GTK widget. Start it with `--accept` and use `sheet-set`, `sheet-get`, and `sheet-goto`. Writer's document view is already in the tree; `text-style` sets weight on a range.

## Demos

Window recordings of unmodified apps. The agent uses the socket, not the pointer.

| clip | app | what happens |
| --- | --- | --- |
| `docs/demo.mp4` | GNOME Text Editor | types `Welcome to 수영 책방 Swimming Bookstore!` |
| `docs/demo-calc.mp4` | GNOME Calculator | clicks `1 + 1 =`, display ends at `2` |
| `docs/demo-writer.mp4` | LibreOffice Writer | types the same line and bolds the bookstore name |
| `docs/demo-calc-sheet.mp4` | LibreOffice Calc | writes cells; the sheet shows 6, 12, and 18 |

```bash
PYTHONPATH=$HOME/demo python3 scripts/record-demo.py
PYTHONPATH=$HOME/demo python3 scripts/record-calc.py
PYTHONPATH=$HOME/demo python3 scripts/record-writer.py
PYTHONPATH=$HOME/demo python3 scripts/record-calc-sheet.py
```

The socket is mode `0600` in the user runtime dir. It only exposes what the app already exposes to a screen reader. Custom-drawn canvases, games, and GTK 2 are out of scope.
