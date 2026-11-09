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
