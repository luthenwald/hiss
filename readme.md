# hiss

Grey ghost text for Helix, from Claude. While you type in insert mode, hiss asks the Anthropic API what comes next and shows it beside the cursor in the inlay hint tone. Tab takes it; typing on lets it go.

hiss needs Helix with the Steel plugin system (mattwparas/helix, branch `steel-event-system`) and an Anthropic API key in `HISS_ANTHROPIC_API_KEY` or `ANTHROPIC_API_KEY`.

It comes in two halves. `hiss.scm` is a Steel module that watches the keys and draws the suggestion. `libhiss` is a small Rust library, loaded by Steel, that keeps one connection to the API open and streams the answer back, so a suggestion can start to appear while you are still pausing.

The suggestion is drawn as an inlay hint, so it takes the color of your theme's `ui.virtual.inlay-hint` scope. If your theme leaves that scope unset, the ghost looks like ordinary text; a small theme that inherits yours fixes it:

```toml
# ~/.config/helix/themes/base16_terminal_hiss.toml
inherits = "base16_terminal"
"ui.virtual.inlay-hint" = "gray"
```

Then set `theme = "base16_terminal_hiss"` in `config.toml`.

To load it, add this to `~/.config/helix/init.scm`:

```scheme
(require "helix/keymaps.scm")
(require "hiss/hiss.scm")

(add-global-keybinding
   (hash "insert"
      (hash "tab" ":hiss-accept"
            "C-space" ":hiss-try")))
```

hiss starts on. `:hiss-toggle`, `:hiss-on` and `:hiss-off` switch it, and `:hiss-clear` removes the suggestion. When you type what the suggestion shows, it shortens as you go. hiss waits 150 ms after your last edit before it asks; `(hiss-configure #:debounce 250)` in `init.scm` changes that.
