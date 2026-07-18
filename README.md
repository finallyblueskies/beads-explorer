# beads explorer

`be` is a dead simple, fast terminal explorer for [beads_rust](https://github.com/Dicklesworthstone/beads_rust) issue graphs. It asks the installed `br` command for JSON data, so it uses the same workspace discovery and database semantics as the main beads CLI.

https://github.com/user-attachments/assets/90893790-e9f1-4815-9cd2-ac79fdebd81f

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/finallyblueskies/beads-explorer/main/install.sh | sh
```

The script builds `be` with cargo (cloning the repo to a temp dir first, or in place when run from a checkout) and installs it into `~/.local/bin` — override with `curl ... | BIN_DIR=/some/path sh`. Uninstall the same way with `uninstall.sh`, or use `cargo install --git https://github.com/finallyblueskies/beads-explorer` if you'd rather skip the script.

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

## Planned improvements

- Quick edit issue status, description, title from task view
