# hiss

Grey ghost text for Helix, from Claude. While you type in insert mode, hiss asks the Anthropic API what comes next and shows it beside the cursor in the inlay hint tone. Tab takes it; typing on lets it go.

hiss needs Helix with the Steel plugin system (mattwparas/helix, branch `steel-event-system`) and an Anthropic API key in `HISS_ANTHROPIC_API_KEY` or `ANTHROPIC_API_KEY`.

It comes in two halves. `hiss.scm` is a Steel module that watches the keys and draws the suggestion. `libhiss` is a small Rust library, loaded by Steel, that keeps one connection to the API open and streams the answer back, so a suggestion can start to appear while you are still pausing.

## Installing

Build and install with `just install` from a clone of this repository. It needs Rust and `just`. It puts the library in Steel's `native/` folder and the Steel module in `cogs/hiss/`, inside Steel's home: `$STEEL_HOME` when that is set, else `~/.steel` when it exists, else `~/.local/share/steel`.

Then add this to `~/.config/helix/init.scm`:

```scheme
(require "helix/keymaps.scm")
(require "hiss/hiss.scm")

(add-global-keybinding
   (hash "insert"
      (hash "tab" ":hiss-accept"
            "C-space" ":hiss-try")))
```

The library loads once per editor, so restart Helix after installing a new build.

The suggestion is drawn as an inlay hint, so it takes the color of your theme's `ui.virtual.inlay-hint` scope. If your theme leaves that scope unset, the ghost looks like ordinary text; a small theme that inherits yours fixes it:

```toml
# ~/.config/helix/themes/base16_terminal_hiss.toml
inherits = "base16_terminal"
"ui.virtual.inlay-hint" = "gray"
```

Then set `theme = "base16_terminal_hiss"` in Helix's `config.toml`.

## Using it

hiss starts on. `:hiss-toggle`, `:hiss-on` and `:hiss-off` switch it, and `:hiss-clear` removes the suggestion. When you type what the suggestion shows, it shortens as you go. `:hiss-try` asks right away and shows how long the answer took.

## Settings

Settings live in `~/.config/hiss/config.toml` (under `$XDG_CONFIG_HOME` when that is set). Every line is optional; [`examples/config.toml`](examples/config.toml) lists them all with their defaults:

| setting | default | what it does |
|---|---|---|
| `enabled` | `true` | suggestions are on when the editor starts |
| `debounce` | `250` | milliseconds to wait after an edit before asking |
| `log` | `true` | write timings to the log (below) |
| `retries` | `1` | times to ask again after an API or network error |
| `prose_retries` | `1` | times to ask again after a reply that was only prose |
| `model` | `"claude-haiku-5-5"` | the model that writes suggestions |
| `max_tokens` | `96` | the most tokens one suggestion may use |
| `context_before` | `8000` | characters of the file sent before the cursor |
| `context_after` | `2000` | characters sent after it |

hiss reads the file when the editor starts. A line it cannot read keeps its default, and the status line says which line it was.

hiss writes one line per request to `~/.local/state/hiss/hiss.log` (under `$XDG_STATE_HOME` when that is set): how long the answer took, how many tokens it used, whether it was shown, cancelled or taken, and how much context was sent. The log never holds your code, file names or key.

## Uninstalling

1. Remove the hiss lines from `~/.config/helix/init.scm`, and the theme you made for it, if any, from `~/.config/helix/themes/`. Set `theme` in Helix's `config.toml` back to your old theme.
2. Remove what `just install` put in Steel's home (see Installing for where that is): `native/libhiss.dylib` (`native/libhiss.so` on Linux) and the folder `cogs/hiss/`.
3. Remove the settings folder `~/.config/hiss/` and the log folder `~/.local/state/hiss/` (or the same folders under `$XDG_CONFIG_HOME` and `$XDG_STATE_HOME`).
4. Delete the clone of this repository; its `target/` folder holds the build.

On a default setup this is:

```sh
rm -f ~/.local/share/steel/native/libhiss.dylib
rm -rf ~/.local/share/steel/cogs/hiss ~/.config/hiss ~/.local/state/hiss
```

hiss keeps nothing else: the API key stays in your environment, and hiss writes no other files.
