# Changelog

## Unreleased

- New: while you type in insert mode, hiss suggests what comes next as grey text after the cursor, over as many lines as it needs, indented the way your file is and lined up with the lines above it. In the middle of a word, the suggestion finishes the word; on an empty line, it starts at the column it belongs at, even left of the cursor.
- New: tab takes the suggestion, and indents as before when there is none. Typing what the suggestion shows keeps it and shortens it.
- New: `:hiss-toggle`, `:hiss-on` and `:hiss-off` switch suggestions, and `:hiss-try` asks right away and shows how long the answer took.
- New: settings in `~/.config/hiss/config.toml`: whether hiss starts on, how long it waits after an edit before asking (250 ms), the log, retries, the model, the reply length and how much of the file is sent.
- New: after an API or network error that may pass, hiss asks once more by itself; `retries` and `prose_retries` set how often.
- New: hiss keeps a log of how long each suggestion took in `~/.local/state/hiss/hiss.log`, with times and sizes only; `log = false` turns it off.
- New: the readme explains how to install and uninstall hiss, and lists every file it creates.
