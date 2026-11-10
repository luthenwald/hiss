#!/bin/sh
# Builds hiss and installs it into Steel's home, or removes it again.
#
#    ./install.sh [--profile release|quick]
#    ./install.sh --uninstall
#
# Steel's home is $STEEL_HOME when that is set, else ~/.steel when it
# exists, else $XDG_DATA_HOME/steel (~/.local/share/steel by default). The
# library goes into its native/ folder and the Steel module into cogs/hiss/.

set -eu

profile=release
uninstall=no

usage() {
   sed -n '2,5p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
   case "$1" in
      --profile)
         [ $# -ge 2 ] || { echo "--profile needs a name." >&2; exit 1; }
         profile="$2"
         shift 2
         ;;
      --profile=*)
         profile="${1#--profile=}"
         shift
         ;;
      --uninstall)
         uninstall=yes
         shift
         ;;
      -h | --help)
         usage
         exit 0
         ;;
      *)
         echo "Unknown option: $1" >&2
         usage >&2
         exit 1
         ;;
   esac
done

if [ -n "${STEEL_HOME:-}" ]; then
   home="$STEEL_HOME"
elif [ -d "$HOME/.steel" ]; then
   home="$HOME/.steel"
else
   home="${XDG_DATA_HOME:-$HOME/.local/share}/steel"
fi

case "$(uname -s)" in
   Darwin) dylib=libhiss.dylib ;;
   *) dylib=libhiss.so ;;
esac

if [ "$uninstall" = yes ]; then
   for path in "$home/native/libhiss.dylib" "$home/native/libhiss.so" "$home/cogs/hiss"; do
      if [ -e "$path" ]; then
         rm -rf "$path"
         echo "Removed $path"
      fi
   done
   echo "Left in place: the hiss lines in ~/.config/helix/init.scm, the settings in"
   echo "~/.config/hiss and the log in ~/.local/state/hiss."
   exit 0
fi

command -v cargo > /dev/null 2>&1 || {
   echo "cargo was not found. Install Rust first: https://rustup.rs" >&2
   exit 1
}

root="$(cd "$(dirname "$0")" && pwd)"
[ -f "$root/Cargo.toml" ] && [ -f "$root/hiss.scm" ] || {
   echo "Run this script from a copy of the hiss project." >&2
   exit 1
}

echo "Building libhiss ($profile)..."
(cd "$root" && cargo build --quiet --profile "$profile")

built="$root/target/$profile/$dylib"
[ -f "$built" ] || { echo "The build did not give $built." >&2; exit 1; }

# A library that does not load would make Steel's loader take the editor
# down at start, so it is checked before it replaces a working one.
if command -v python3 > /dev/null 2>&1; then
   python3 -I -c 'import ctypes, sys; ctypes.CDLL(sys.argv[1])' "$built" 2> /dev/null || {
      echo "The built library does not load, so it is not installed." >&2
      exit 1
   }
else
   echo "python3 was not found, so the check that the library loads was skipped."
fi

mkdir -p "$home/native" "$home/cogs/hiss"
# Unlinked first, so a running editor keeps the copy it loaded.
rm -f "$home/native/$dylib"
cp "$built" "$home/native/$dylib"
cp "$root/hiss.scm" "$root/cog.scm" "$home/cogs/hiss/"

echo "Installed $home/native/$dylib"
echo "Installed $home/cogs/hiss/hiss.scm"
echo "Installed $home/cogs/hiss/cog.scm"
echo "Restart Helix to load the new build."
