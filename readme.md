# hiss

Grey ghost text for Helix, from Claude. While you type in insert mode, hiss asks the Anthropic API what comes next and shows it beside the cursor in the inlay hint tone. Tab takes it; typing on lets it go.

It comes in two halves. `hiss.scm` is a Steel module that watches the keys and draws the suggestion. `libhiss` is a small Rust library, loaded by Steel, that keeps one connection to the API open and streams the answer back, so a suggestion can start to appear while you are still pausing.

## Requirements

- Helix with the Steel plugin system: the fork at [mattwparas/helix](https://github.com/mattwparas/helix), branch `steel-event-system`. Its `STEEL.md` explains how to build it; in a clone of the fork on that branch, `cargo xtask steel` installs `hx` with Steel built in. Upstream Helix has no plugin system and cannot run hiss.
- Rust, to build the library.
- An Anthropic API key in `HISS_ANTHROPIC_API_KEY` or `ANTHROPIC_API_KEY`, set in the environment Helix starts from.

The library has to be built against the same versions of `steel-core`, `abi_stable` and `async-ffi` as the editor, or Steel cannot load it. hiss pins them in `Cargo.toml` to what the fork used at revision 424add10. If you run a newer fork, copy the versions from the fork's `Cargo.lock` into `Cargo.toml` and build again.

## Installing

From a clone of this repository:

```sh
./install.sh
```

It builds the library, checks that it loads, and copies it to Steel's `native/` folder, with the Steel module in `cogs/hiss/`. Steel's home is `$STEEL_HOME` when that is set, else `~/.steel` when it exists, else `~/.local/share/steel`. The script prints every path it writes.

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

The suggestion is drawn as an inlay hint, so it takes the color of your theme's `ui.virtual.inlay-hint` scope. If your theme leaves that scope unset, the ghost looks like ordinary text. A small theme that inherits yours fixes it:

```toml
# ~/.config/helix/themes/base16_terminal_hiss.toml
inherits = "base16_terminal"
"ui.virtual.inlay-hint" = "gray"
```

Then set `theme = "base16_terminal_hiss"` in Helix's `config.toml`.

## Using it

hiss starts on. A short pause in insert mode brings a suggestion, over as many lines as it needs. When you type what it shows, it shortens as you go; when you type something else, it goes away and hiss asks again after the next pause.

| command | what it does |
|---|---|
| `:hiss-accept` | inserts the suggestion, or indents as usual when there is none (tab in the example above) |
| `:hiss-try` | asks right away and shows how long the answer took (C-space above) |
| `:hiss-clear` | removes the suggestion |
| `:hiss-on`, `:hiss-off`, `:hiss-toggle` | switch suggestions on and off |

hiss remembers its last 64 suggestions while the editor runs. When an undo or a retyped character brings back text it has already answered, the suggestion returns at once without asking again. `:hiss-try` always asks.

## What hiss sends

Each request carries the text around the cursor (up to `context_before` characters before it and `context_after` after it, cut at line ends), the file's path relative to Helix's working directory (or only its name when the file lies outside it), and the language. It goes to the Anthropic API under your key and is billed to it; see [Anthropic's pricing](https://www.anthropic.com/pricing) for the model you use. Nothing else leaves your machine.

hiss asks only after a pause, and drops a request as soon as you type on, so most requests stop early; a dropped request may still be billed for what it sent. In a ten-minute session of ordinary editing, with a 150 ms pause, hiss sent about 200 requests, and one suggestion in ten was taken. At the median, a request sent about 440 tokens and got about 15 back.

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

## The log

hiss writes one line per request to `~/.local/state/hiss/hiss.log` (under `$XDG_STATE_HOME` when that is set): how long the answer took, how many tokens it used, whether it was shown, cancelled, taken or remembered, and how much context was sent. The log never holds your code, file names or key. `log = false` turns it off.

## When something is wrong

- No suggestion appears. Run `:hiss-try`: the status line shows the time the answer took, or what went wrong, such as `hiss: no api key` when Helix was started without the key in its environment. A setting hiss could not read is named in the status line the first time you enter insert mode. The log shows whether requests are sent and how they end.
- Suggestions stopped after an error. When the API turns down every request in the same way, for a missing or wrong key or a `model` it does not know, hiss says so once and stops asking. Fix the cause, then run `:hiss-try` or `:hiss-on`, and hiss asks again.
- The suggestion looks like ordinary text. Your theme has no `ui.virtual.inlay-hint` color; see the end of Installing.
- Helix stops at start after a fork update. The library no longer matches the editor; see Requirements for the versions to copy, then run `./install.sh` again.
- The suggestion vanishes now and then. With `lsp.display-inlay-hints = true` in Helix's config, a language server's hints are redrawn from time to time, and the redraw can take the suggestion with it. hiss asks again after your next edit.

## Uninstalling

1. Run `./install.sh --uninstall` from the clone. It removes `native/libhiss.dylib` (`native/libhiss.so` on Linux) and the folder `cogs/hiss/` from Steel's home.
2. Remove the hiss lines from `~/.config/helix/init.scm`, and the theme you made for it, if any, from `~/.config/helix/themes/`. Set `theme` in Helix's `config.toml` back to your old theme.
3. Remove the settings folder `~/.config/hiss/` and the log folder `~/.local/state/hiss/` (or the same folders under `$XDG_CONFIG_HOME` and `$XDG_STATE_HOME`).
4. Delete the clone of this repository; its `target/` folder holds the build.

Without the clone, on a default setup, the first and third steps are:

```sh
rm -f ~/.local/share/steel/native/libhiss.dylib
rm -rf ~/.local/share/steel/cogs/hiss ~/.config/hiss ~/.local/state/hiss
```

hiss keeps nothing else: the API key stays in your environment, and hiss writes no other files.

## License

MIT, see [`license`](license).
