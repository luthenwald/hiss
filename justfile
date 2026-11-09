# Lists the recipes.
_default:
   @just --list

# Builds a debug dylib.
build:
   cargo build

# Runs the tests.
test:
   cargo test

# Builds the release dylib, with LTO.
release:
   cargo build --release

# Builds an optimized dylib quickly, without LTO, for trying things out.
quick:
   cargo build --profile quick

# Checks that the Rust code is formatted.
fmt-check:
   cargo fmt --check

# Steel's home, found the way Steel finds it.
steel-home := env("STEEL_HOME", if path_exists(home_directory() / ".steel") == "true" { home_directory() / ".steel" } else { env("XDG_DATA_HOME", home_directory() / ".local/share") / "steel" })
dylib := if os() == "macos" { "libhiss.dylib" } else { "libhiss.so" }

# Installs libhiss and hiss.scm into Steel's home (the old dylib is unlinked first, so a running editor keeps its copy).
install profile="release":
   cargo build --profile {{profile}}
   mkdir -p "{{steel-home}}/native" "{{steel-home}}/cogs/hiss"
   rm -f "{{steel-home}}/native/{{dylib}}"
   cp "target/{{profile}}/{{dylib}}" "{{steel-home}}/native/"
   cp hiss.scm cog.scm "{{steel-home}}/cogs/hiss/"

# Opens the Steel fork with hiss loaded, using examples/ as its Steel config.
hx *args: (install "quick")
   HELIX_STEEL_CONFIG="{{justfile_directory()}}/examples" hxs {{args}}
