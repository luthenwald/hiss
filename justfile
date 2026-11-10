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

# Builds libhiss and installs it with hiss.scm into Steel's home (see install.sh).
install profile="release":
   ./install.sh --profile {{profile}}

# Opens the Steel fork with hiss loaded, using examples/ as its Steel config.
hx *args: (install "quick")
   HELIX_STEEL_CONFIG="{{justfile_directory()}}/examples" hxs {{args}}

# Completes a fixture through the API without Helix and prints the timings; flags such as `--before 2000,8000` or `--cache off,on` compare variants run by run.
bench fixture="tests/fixtures/mean.rs" runs="5" *args:
   cargo run --quiet --profile quick --bin complete -- --runs {{runs}} {{args}} {{fixture}}

# Judges replies for every fixture (prose, indentation) and prints the clean counts.
quality runs="5" *args:
   cargo run --quiet --profile quick --bin complete -- --check --runs {{runs}} {{args}} tests/fixtures/*
