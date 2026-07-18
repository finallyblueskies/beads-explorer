# beads explorer

`be` is a small, fast terminal explorer for [beads_rust](https://github.com/Dicklesworthstone/beads_rust) issue graphs. It asks the installed `br` command for JSON data, so it uses the same workspace discovery and database semantics as the main beads CLI.

## Install

```sh
./install.sh
```

This builds a release binary and installs `be` into `~/.local/bin` (override with `BIN_DIR=/some/path ./install.sh`). Remove it later with `./uninstall.sh`. If you prefer cargo, `cargo install --path .` works too.

Then run `be` anywhere that `br list` works. The tree shows issues whose status is `open`; non-open dependencies remain available when navigating through Task View. Use `be --db path/to/beads.db` to select a database explicitly.

## Navigation

| Key | Tree | Task view |
| --- | --- | --- |
| `j` / `k`, arrows | Move | Select dependency |
| `h` / `l`, Left / Right | Fold / expand | — |
| `Tab` | Toggle fold | — |
| `Enter` | Open task | Open dependency |
| `/` | Fuzzy go-to by issue ID | — |
| `Backspace` | — | Previous task/tree |
| `Esc` | Quit | Return to tree |
| `q` | Quit | Quit |

While go-to is open, type any part of an issue ID; matching is case-insensitive and fuzzy, so the characters only need to appear in order. Search filters only the rows currently visible in the tree—children of collapsed issues are excluded. Use the arrow keys to select a match, `Enter` to open it, or `Esc` to cancel.
