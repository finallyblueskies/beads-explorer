# beads explorer

`be` is a dead simple, fast terminal explorer for [beads](https://github.com/gastownhall/beads) issue graphs. It asks the installed `bd` command for JSON data, so it uses the same workspace discovery and database semantics as the main beads CLI.

https://github.com/user-attachments/assets/9a4cd863-e8f6-44a6-9ff6-37f91a27a9ae

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/finallyblueskies/beads-explorer/main/install.sh | sh
```

The script builds `be` with cargo (cloning the repo to a temp dir first, or in place when run from a checkout) and installs it into `~/.local/bin` — override with `curl ... | BIN_DIR=/some/path sh`. To uninstall, pipe `uninstall.sh` through `sh` the same way.

If you'd rather skip the script: `cargo install --git https://github.com/finallyblueskies/beads-explorer` installs to `~/.cargo/bin` (remove with `cargo uninstall beads-explorer`).

Then run `be` anywhere that `bd list` works. The tree shows issues whose status is `open`; non-open dependencies remain available when navigating through Task View. Use `be --db path/to/.beads` to select a database explicitly, or `--bd path/to/bd` (env: `BEADS_EXPLORER_BD`) to point at a specific `bd` executable.

## Navigation

| Key | Tree | Task view |
| --- | --- | --- |
| `j` / `k`, arrows | Move | Select dependency |
| `h` / `l`, Left / Right | Fold / expand | — |
| `Tab` | Toggle fold | — |
| `Enter` | Open task | Open dependency |
| `e` | — | Edit description in `$EDITOR` |
| `x`, then `y` | Close selected issue after confirmation | Close issue after confirmation |
| `/` | Fuzzy go-to by issue ID | — |
| `Backspace` | — | Previous task/tree |
| `Esc` | Quit | Return to tree |
| `q` | Quit | Quit |

While go-to is open, type any part of an issue ID; matching is case-insensitive and fuzzy, so the characters only need to appear in order. Search filters only the rows currently visible in the tree—children of collapsed issues are excluded. Use the arrow keys to select a match, `Enter` to open it, or `Esc` to cancel.

## Planned improvements

- Quick edit issue status and title from task view
