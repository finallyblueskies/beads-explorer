# beads explorer

`be` is a dead simple, fast terminal explorer for [beads](https://github.com/gastownhall/beads) issue graphs. It asks the installed `bd` command for JSON data, so it uses the same workspace discovery and database semantics as the main beads CLI.

https://github.com/user-attachments/assets/206a634c-8ced-4588-ae28-14b8174a9a3a

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
| `Enter` | Open task (`+ Create New` entry: add a top-level issue) | Open dependency |
| `+` | Add a child to the selected issue | Add a child to this issue |
| `e` / `ee` | Edit description in `$EDITOR` | Edit description in `$EDITOR` |
| `et` | Edit title in `$EDITOR` | Edit title in `$EDITOR` |
| `s` | Set status of the selected issue | Set status of this issue |
| `p` | Set priority of the selected issue | Set priority of this issue |
| `x`, then `y` | Close selected issue after confirmation | Close issue after confirmation |
| `/` | Fuzzy go-to by issue ID | — |
| `Backspace` | — | Previous task/tree |
| `Esc` | Quit | Return to tree |
| `q` | Quit | Quit |

Editing keys are the same everywhere: they act on the issue selected in the tree or on the issue open in Task View. `s` and `p` open a small menu preselecting the current status/priority — `j`/`k` to choose, `Enter` to apply via `bd update`, `Esc` to cancel.

While go-to is open, type any part of an issue ID; matching is case-insensitive and fuzzy, so the characters only need to appear in order. Search filters only the rows currently visible in the tree—children of collapsed issues are excluded. Use the arrow keys to select a match, `Enter` to open it, or `Esc` to cancel.

Press `+` to create a child issue at the current location, or press `Enter` on the `+ Create New` entry at the top of the tree to create a top-level issue (also available when the database is empty). The flow collects a title, description, issue type, and priority (P1 by default). Type text directly or press `e` on an empty title/description to use `$VISUAL`/`$EDITOR`; use `j`/`k` or the arrow keys for selections. `Esc` can cancel from any step after confirmation.
