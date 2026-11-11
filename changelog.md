# Changelog

## Unreleased

## Grey ghost text in Helix

0.1.0 · 2026-11-12 · `twyputpv::kwppskql`

- New: while you type in insert mode, hiss suggests what comes next as grey text after the cursor, over as many lines as it needs, indented the way your file is and lined up with the lines above it. In the middle of a word, the suggestion finishes the word; on an empty line, it starts at the column it belongs at, even left of the cursor.
- New: when an undo or a retyped character brings back text hiss has already answered, the suggestion returns at once, without asking again.
- New: tab takes the suggestion, and indents as before when there is none. Typing what the suggestion shows keeps it and shortens it. When you type on before a suggestion has arrived and it starts with what you typed, its rest is shown, with no second request. After tab, and when you enter insert mode, the next suggestion is asked for at once.
- New: `:hiss-toggle`, `:hiss-on` and `:hiss-off` switch suggestions, and `:hiss-try` asks right away and shows how long the answer took.
- New: settings in `~/.config/hiss/config.toml`: whether hiss starts on, how long it waits after an edit before asking (250 ms), the log, retries, the model, the reply length and how much of the file is sent.
- New: after an API or network error that may pass, or a reply that was only prose, hiss asks once more by itself; `retries` and `prose_retries` set how often. When such errors keep coming, as they do without a network, hiss says so once and waits before it asks again, 30 seconds at first and up to five minutes.
- New: hiss keeps a log of how long each suggestion took in `~/.local/state/hiss/hiss.log`, with times and sizes only, and moves it to `hiss.log.1` once it passes 1 MB; `log = false` turns it off.
- New: hiss never sends files that usually hold secrets, such as `.env`, key files and `~/.netrc`, and the `exclude` setting keeps more files out.
- New: `./install.sh` builds hiss and puts it where Steel looks for it, and `./install.sh --uninstall` removes it again.
