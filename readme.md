# hiss

Grey ghost text for Helix, from Claude. While you type in insert mode, hiss asks the Anthropic API what comes next and shows it beside the cursor in the inlay hint tone. Tab takes it; typing on lets it go.

It comes in two halves. `hiss.scm` is a Steel module that watches the keys and draws the suggestion. `libhiss` is a small Rust library, loaded by Steel, that keeps one connection to the API open and streams the answer back, so a suggestion can start to appear while you are still pausing.

## Installing

hiss runs in a build of Helix with the Steel plugin system, which lives in a fork of Helix. Upstream Helix has no plugin system and cannot run hiss. The steps below install the fork as `hx`, then hiss into it.

### What you need

- Rust, installed through [rustup](https://rustup.rs).
- git and a C compiler, which Helix uses to build its tree-sitter grammars. On macOS, `xcode-select --install` brings both. On Linux, install `git` and `gcc` or `clang` from your package manager.
- An Anthropic API key, described below.
- python3, if you have it. The install script uses it to check that the library loads before it copies it.

### Helix with Steel

The fork is [mattwparas/helix](https://github.com/mattwparas/helix), on the branch `steel-event-system`. Clone it, check out the revision hiss was built against, and install it:

```sh
git clone --depth 1 --branch steel-event-system https://github.com/mattwparas/helix.git
cd helix
git checkout 424add10
cargo install --path helix-term --features steel,git --locked
```

The first build installs the Rust version the fork asks for, then fetches and compiles the grammars, and takes several minutes. It puts `hx` in `~/.cargo/bin`, which should come before any other `hx` on your `PATH`.

Helix finds its themes, grammars and queries in the clone's `runtime/` folder, named by `HELIX_RUNTIME`. Set it to that folder's full path (`pwd` in the clone shows where it is) in your shell's profile, `~/.zshrc` or `~/.bashrc`:

```sh
export HELIX_RUNTIME=/path/to/helix/runtime
```

or, for fish, in `~/.config/fish/config.fish`:

```fish
set -gx HELIX_RUNTIME /path/to/helix/runtime
```

In a new shell, `hx --version` should end in `(424add10)`, and `hx --health` should list the clone's `runtime` folder among its runtime directories. The fork reads the same `~/.config/helix` as Helix, so your settings, themes and languages carry over.

hiss is built against the versions of `steel-core`, `abi_stable` and `async-ffi` that this revision of the fork uses, and Steel loads it only when they match. A newer revision of the fork may need new versions in hiss; see [Updating](#updating). The fork's `STEEL.md` suggests `cargo xtask steel` in place of `cargo install`. That also installs the Steel tools from Steel's newest revision, which hiss does not need.

To keep another Helix as `hx`, build the fork in its clone without installing it:

```sh
cargo build --release -p helix-term --features steel,git --locked
```

Then start it through a small script on your `PATH`, such as `~/.local/bin/hx-steel`, and leave `HELIX_RUNTIME` out of your profile:

```sh
#!/bin/sh
export HELIX_RUNTIME=/path/to/helix/runtime
exec /path/to/helix/target/release/hx "$@"
```

Make it executable with `chmod +x ~/.local/bin/hx-steel`. Your other `hx` stays as it was, and since plain Helix ignores `init.scm`, both can share `~/.config/helix`.

### hiss

```sh
git clone --depth 1 --branch main https://github.com/luthenwald/hiss.git
cd hiss
./install.sh
```

The script builds the library, checks that it loads, and copies it to Steel's `native/` folder, with the Steel module in `cogs/hiss/`. Steel's home is `$STEEL_HOME` when that is set, else `~/.steel` when it exists, else `~/.local/share/steel`. The script prints every path it writes.

### The API key

Make a key in the [Claude Console](https://platform.claude.com/settings/keys) and put it in the environment Helix starts from, in the same profile as before:

```sh
export ANTHROPIC_API_KEY=sk-ant-...
```

```fish
set -gx ANTHROPIC_API_KEY sk-ant-...
```

hiss reads `HISS_ANTHROPIC_API_KEY` first, so you can give it a key of its own and leave `ANTHROPIC_API_KEY` to other tools. Requests are billed to the key; see [What hiss sends](#what-hiss-sends).

### Loading hiss

Add this to `~/.config/helix/init.scm`, and create the file if it does not exist:

```scheme
(require "helix/keymaps.scm")
(require "hiss/hiss.scm")

(add-global-keybinding
   (hash "insert"
      (hash "tab" ":hiss-accept"
            "C-space" ":hiss-try")))
```

### The ghost's color

The suggestion is drawn as an inlay hint, so it takes the color of your theme's `ui.virtual.inlay-hint` scope. If your theme leaves that scope unset, the ghost looks like ordinary text. A small theme that inherits yours fixes it:

```toml
# ~/.config/helix/themes/base16_terminal_hiss.toml
inherits = "base16_terminal"
"ui.virtual.inlay-hint" = "gray"
```

Then set `theme = "base16_terminal_hiss"` in Helix's `config.toml`.

### Checking it works

Start a new shell, so that it has the key and `HELIX_RUNTIME`, open a source file with `hx`, press `i` and pause. A grey suggestion appears after the cursor, usually within a second, and tab takes it. If nothing appears, run `:hiss-try`: the status line shows how long the answer took, or what went wrong. [When something is wrong](#when-something-is-wrong) lists the usual causes.

## Using it

hiss starts on. A short pause in insert mode brings a suggestion, over as many lines as it needs. After tab takes a suggestion, and when you enter insert mode, hiss asks right away without waiting for the pause. When you type what it shows, it shortens as you go, also when you typed ahead of a suggestion that was still on its way; when you type something else, it goes away and hiss asks again after the next pause.

| command | what it does |
|---|---|
| `:hiss-accept` | inserts the suggestion, or indents as usual when there is none (tab in the example above) |
| `:hiss-try` | asks right away and shows how long the answer took (C-space above) |
| `:hiss-clear` | removes the suggestion |
| `:hiss-on`, `:hiss-off`, `:hiss-toggle` | switch suggestions on and off |

hiss remembers its last 64 suggestions while the editor runs. When an undo or a retyped character brings back text it has already answered, the suggestion returns at once without asking again. `:hiss-try` always asks.

## What hiss sends

Each request carries the text around the cursor (up to `context_before` characters before it and `context_after` after it, cut at line ends), the file's path relative to Helix's working directory (or only its name when the file lies outside it), and the language. It goes to the Anthropic API under your key and is billed to it; see [Anthropic's pricing](https://www.anthropic.com/pricing) for the model you use. Nothing else leaves your machine.

Some files are never sent, since they usually hold secrets: `.env` and `.env.*`, `.envrc`, `*.pem` and `*.key`, SSH keys (`id_rsa*`, `id_ed25519*` and the like), `.netrc`, `.npmrc`, `.pypirc`, `.pgpass`, `.htpasswd`, `.git-credentials`, `.my.cnf`, `credentials` and `*credentials.json`, `secrets.yml` and its `.yaml`, `.json` and `.toml` forms, Terraform's `*.tfvars` and `*.tfstate`, `.kube/config`, `gh/hosts.yml`, and buffers Helix reads as `env`, `pem`, `passwd`, `hosts` or `ssh_client_config`. The `exclude` setting adds patterns of your own. In such a file hiss stays quiet, and `:hiss-try` says the file is never sent.

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
| `exclude` | `""` | more files never to send, as patterns split by commas: `*.secret` matches a file name, `~/notes/*` a path from home, `private/*` that folder anywhere; letters match in either case |

hiss reads the file when the editor starts. A line it cannot read keeps its default, and the status line says which line it was.

## The log

hiss writes one line per request to `~/.local/state/hiss/hiss.log` (under `$XDG_STATE_HOME` when that is set): how long the answer took, how many tokens it used, whether it was shown, cancelled, taken or remembered, whether a suggestion you typed ahead of still fitted, and how much context was sent. The log never holds your code, file names or key. Once it passes 1 MB, hiss moves it to `hiss.log.1` in the same folder the next time Helix starts, replacing the older one. `log = false` turns it off.

## When something is wrong

- No suggestion appears. Run `:hiss-try`: the status line shows the time the answer took, or what went wrong, such as `hiss: no api key` when Helix was started without the key in its environment. A setting hiss could not read is named in the status line the first time you enter insert mode. The log shows whether requests are sent and how they end.
- Suggestions stopped after an error. When the API turns down every request in the same way, for a missing or wrong key or a `model` it does not know, hiss says so once and stops asking. Fix the cause, then run `:hiss-try` or `:hiss-on`, and hiss asks again.
- Suggestions pause after `asking again in 30 s`. The network or the API failed twice in a row, so hiss waits before it asks by itself again, longer each time up to five minutes. `:hiss-try` and `:hiss-on` end the wait.
- The suggestion looks like ordinary text. Your theme has no `ui.virtual.inlay-hint` color; see the end of Installing.
- Helix stops at start after a fork update. The library no longer matches the editor; see [Updating](#updating) for the versions to copy, then run `./install.sh` again.
- The suggestion vanishes now and then. With `lsp.display-inlay-hints = true` in Helix's config, a language server's hints are redrawn from time to time, and the redraw can take the suggestion with it. hiss asks again after your next edit.

## Updating

To update hiss, pull in its clone and install again:

```sh
git pull
./install.sh
```

Steel loads the library once per editor, so restart Helix afterwards.

When you move the fork to a newer revision, build it again as in Installing. hiss then has to match the fork's new versions of `steel-core`, `abi_stable` and `async-ffi`: copy them from the fork's `Cargo.lock` into hiss's `Cargo.toml` (for `steel-core`, the git revision after the `#`), and run `./install.sh` again. With versions that do not match, Helix stops at start when Steel loads the library.

## Uninstalling

1. Run `./install.sh --uninstall` from the clone. It removes `native/libhiss.dylib` (`native/libhiss.so` on Linux) and the folder `cogs/hiss/` from Steel's home.
2. Remove the hiss lines from `~/.config/helix/init.scm`, and the theme you made for it, if any, from `~/.config/helix/themes/`. Set `theme` in Helix's `config.toml` back to your old theme.
3. Remove the settings folder `~/.config/hiss/` and the log folder `~/.local/state/hiss/` (or the same folders under `$XDG_CONFIG_HOME` and `$XDG_STATE_HOME`).
4. Delete the clone of this repository; its `target/` folder holds the build.
5. If you installed the fork only for hiss, run `cargo uninstall helix-term` (or delete the `hx-steel` script when you built it side by side), remove the `HELIX_RUNTIME` line from your shell's profile, and delete the fork's clone. The fork also writes `cogs/helix/` and `lsp/` into Steel's home; when nothing else uses Steel, the whole of Steel's home can go.

Without the clone, on a default setup, the first and third steps are:

```sh
rm -f ~/.local/share/steel/native/libhiss.dylib
rm -rf ~/.local/share/steel/cogs/hiss ~/.config/hiss ~/.local/state/hiss
```

hiss keeps nothing else and writes no other files. The API key stays in your shell's profile; remove its line there if you added it for hiss.

## License

MIT, see [`license`](license).
