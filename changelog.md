# Changelog

## Unreleased

- New: while you type in insert mode, hiss suggests what comes next as grey text after the cursor, over as many lines as it needs, indented the way your file is.
- New: tab takes the suggestion, and indents as before when there is none. Typing what the suggestion shows keeps it and shortens it.
- New: `:hiss-toggle`, `:hiss-on` and `:hiss-off` switch suggestions, and `:hiss-try` asks right away and shows how long the answer took.
- New: `(hiss-configure #:debounce 250)` sets how long hiss waits after an edit before asking.
- New: hiss keeps a log of how long each suggestion took in `~/.local/state/hiss/hiss.log`, with times and sizes only, and `(hiss-configure #:log #f)` turns it off.
