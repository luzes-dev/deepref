#!/usr/bin/env bash
set -euo pipefail

dockerfile=${1:-docker/rust-service.Dockerfile}
version=$(sed -n 's/^ARG PDFIUM_VERSION="\([0-9][0-9]*\)"$/\1/p' "$dockerfile" | head -n1)
url=$(sed -n 's/^ARG PDFIUM_URL="\(.*\)"$/\1/p' "$dockerfile" | head -n1)
sha=$(sed -n 's/^ARG PDFIUM_SHA256="\([0-9a-f]\{64\}\)"$/\1/p' "$dockerfile" | head -n1)

test "$version" = 7881
test "$url" = 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-linux-x64.tgz'
test "$sha" = 1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d
grep -Fq 'sha256sum --check --strict' "$dockerfile"
# The Dockerfile must retain this literal variable reference.
# shellcheck disable=SC2016
grep -Fq 'grep --fixed-strings --line-regexp "BUILD=${PDFIUM_VERSION}"' "$dockerfile"
grep -Fq 'COPY --from=pdfium' "$dockerfile"
grep -Fq 'ENV PDFIUM_LIBRARY_PATH=/usr/local/lib/libpdfium.so' "$dockerfile"

printf 'Pdfium pin %s verified in %s\n' "$version" "$dockerfile"
