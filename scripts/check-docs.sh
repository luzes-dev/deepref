#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'documentation check failed: %s\n' "$*" >&2
  exit 1
}

mapfile -d '' markdown_files < <(
  find README.md CONTRIBUTING.md SECURITY.md docs apps/web/README.md \
    -type f -name '*.md' -print0
)

repository_root=$(pwd -P)
for file in "${markdown_files[@]}"; do
  while IFS= read -r destination; do
    destination=${destination#<}
    destination=${destination%>}
    destination=${destination%% *}
    case "$destination" in
      ''|\#*|http://*|https://*|mailto:*) continue ;;
    esac
    path=${destination%%#*}
    path=${path%%\?*}
    [[ -n $path ]] || continue
    resolved=$(realpath -m "$(dirname "$file")/$path")
    [[ $resolved == "$repository_root"/* ]] || fail "$file links outside the repository: $destination"
    [[ -e $resolved ]] || fail "$file has a broken internal link: $destination"
  done < <(perl -ne 'while (/\[[^\]]+\]\(([^)]+)\)/g) { print "$1\n" }' "$file")
done

printf 'documentation checks passed: links verified across markdown files\n'
