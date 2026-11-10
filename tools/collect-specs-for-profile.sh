#!/usr/bin/env bash
#
# Collect specs/ and proto/ files from each component referenced by a
# certus-server-yaml profile, plus the server's own proto definitions.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROFILES_DIR="$REPO_ROOT/apps/certus-server-yaml/profiles"
COMPONENTS_DIR="$REPO_ROOT/components"
SERVER_YAML_DIR="$REPO_ROOT/apps/certus-server-yaml"

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS] <profile> <target-dir>

Collect specs/ and proto/ from each component referenced by a
certus-server-yaml profile, plus the server proto definitions.

Arguments:
  <profile>      Profile name (without .yaml extension) or path to a YAML file
  <target-dir>   Directory to collect specs into

Options:
  -s, --symlink  Create symlinks instead of copying
  -l, --list     List available profiles and exit
  -h, --help     Show this help

Examples:
  $(basename "$0") full ./collected-specs
  $(basename "$0") --symlink full-p2p /tmp/certus-specs
EOF
    exit "${1:-0}"
}

list_profiles() {
    echo "Available profiles:"
    for f in "$PROFILES_DIR"/*.yaml; do
        name=$(basename "$f" .yaml)
        desc=$(grep -A1 '^profile:' "$f" | grep 'description:' | sed 's/.*description: *"\(.*\)"/\1/' || echo "")
        printf "  %-20s %s\n" "$name" "$desc"
    done
}

USE_SYMLINK=0
PROFILE=""
TARGET_DIR=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -s|--symlink)   USE_SYMLINK=1; shift ;;
        -l|--list)      list_profiles; exit 0 ;;
        -h|--help)      usage 0 ;;
        -*)             echo "Unknown option: $1" >&2; usage 1 ;;
        *)
            if [[ -z "$PROFILE" ]]; then
                PROFILE="$1"
            elif [[ -z "$TARGET_DIR" ]]; then
                TARGET_DIR="$1"
            else
                echo "Error: unexpected argument: $1" >&2
                usage 1
            fi
            shift
            ;;
    esac
done

if [[ -z "$PROFILE" || -z "$TARGET_DIR" ]]; then
    echo "Error: both <profile> and <target-dir> are required." >&2
    usage 1
fi

# Resolve profile file
if [[ -f "$PROFILE" ]]; then
    PROFILE_FILE="$PROFILE"
elif [[ -f "$PROFILES_DIR/$PROFILE.yaml" ]]; then
    PROFILE_FILE="$PROFILES_DIR/$PROFILE.yaml"
else
    echo "Error: profile '$PROFILE' not found." >&2
    echo "Available profiles:"
    for f in "$PROFILES_DIR"/*.yaml; do
        echo "  $(basename "$f" .yaml)"
    done
    exit 1
fi

# Extract unique crate names from the profile
CRATES=$(grep '^\s*crate:' "$PROFILE_FILE" | sed 's/.*crate:\s*//' | sort -u)

if [[ -z "$CRATES" ]]; then
    echo "Error: no components found in profile '$PROFILE_FILE'" >&2
    exit 1
fi

# Create target directory
mkdir -p "$TARGET_DIR"

# Collect specs and proto from each component
PROFILE_NAME=$(basename "$PROFILE_FILE" .yaml)
echo "Profile: $PROFILE_NAME"
echo "Target:  $TARGET_DIR"
echo ""

for crate in $CRATES; do
    component_dir="$COMPONENTS_DIR/$crate"
    if [[ ! -d "$component_dir" ]]; then
        echo "  [SKIP] $crate (component not found)"
        continue
    fi

    specs_src="$component_dir/specs"
    proto_src="$component_dir/proto"

    if [[ ! -d "$specs_src" && ! -d "$proto_src" ]]; then
        echo "  [SKIP] $crate (no specs/ or proto/)"
        continue
    fi

    dest="$TARGET_DIR/$crate"
    mkdir -p "$dest"

    if [[ -d "$specs_src" ]]; then
        if [[ $USE_SYMLINK -eq 1 ]]; then
            ln -sfn "$(realpath "$specs_src")" "$dest/specs"
            echo "  [LINK] $crate/specs"
        else
            cp -a "$specs_src" "$dest/specs"
            echo "  [COPY] $crate/specs"
        fi
    fi

    if [[ -d "$proto_src" ]]; then
        if [[ $USE_SYMLINK -eq 1 ]]; then
            ln -sfn "$(realpath "$proto_src")" "$dest/proto"
            echo "  [LINK] $crate/proto"
        else
            cp -a "$proto_src" "$dest/proto"
            echo "  [COPY] $crate/proto"
        fi
    fi
done

# Collect certus-server-yaml proto definitions
SERVER_PROTO="$SERVER_YAML_DIR/proto"
if [[ -d "$SERVER_PROTO" ]]; then
    dest="$TARGET_DIR/certus-server-yaml"
    mkdir -p "$dest"
    if [[ $USE_SYMLINK -eq 1 ]]; then
        ln -sfn "$(realpath "$SERVER_PROTO")" "$dest/proto"
        echo "  [LINK] certus-server-yaml/proto"
    else
        cp -a "$SERVER_PROTO" "$dest/proto"
        echo "  [COPY] certus-server-yaml/proto"
    fi
fi

# Collect certus-server specs and proto
CERTUS_SERVER_DIR="$REPO_ROOT/apps/certus-server"
if [[ -d "$CERTUS_SERVER_DIR/specs" || -d "$CERTUS_SERVER_DIR/proto" ]]; then
    dest="$TARGET_DIR/certus-server"
    mkdir -p "$dest"
    if [[ -d "$CERTUS_SERVER_DIR/specs" ]]; then
        if [[ $USE_SYMLINK -eq 1 ]]; then
            ln -sfn "$(realpath "$CERTUS_SERVER_DIR/specs")" "$dest/specs"
            echo "  [LINK] certus-server/specs"
        else
            cp -a "$CERTUS_SERVER_DIR/specs" "$dest/specs"
            echo "  [COPY] certus-server/specs"
        fi
    fi
    if [[ -d "$CERTUS_SERVER_DIR/proto" ]]; then
        if [[ $USE_SYMLINK -eq 1 ]]; then
            ln -sfn "$(realpath "$CERTUS_SERVER_DIR/proto")" "$dest/proto"
            echo "  [LINK] certus-server/proto"
        else
            cp -a "$CERTUS_SERVER_DIR/proto" "$dest/proto"
            echo "  [COPY] certus-server/proto"
        fi
    fi
fi

# Include the profile YAML itself
cp "$PROFILE_FILE" "$TARGET_DIR/"
echo "  [COPY] $(basename "$PROFILE_FILE")"

echo ""
echo "Done. Collected into: $TARGET_DIR"
