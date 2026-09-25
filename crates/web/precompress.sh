#!/usr/bin/env bash
# Trunk post-build hook: writes .br and .gz beside each text and wasm asset,
# so the server can send the smallest encoding without compressing on every
# request. Brotli at quality 11 is about 20% smaller than the on-the-fly
# default, and far too slow to run per request.
#
# Release builds only. Without the brotli CLI it says so and carries on; the
# server then compresses on the fly, as before.
set -euo pipefail

[[ "${TRUNK_PROFILE:-}" == "release" ]] || exit 0
dir="${TRUNK_STAGING_DIR:?trunk sets this for hooks}"

if ! command -v brotli >/dev/null; then
  echo "precompress: brotli not found; assets will be compressed per request" >&2
  exit 0
fi

shopt -s nullglob
for file in "$dir"/*.wasm "$dir"/*.js "$dir"/*.css "$dir"/*.html "$dir"/*.webmanifest "$dir"/*.svg "$dir"/icons/*.svg "$dir"/brand-icons/*.svg; do
  brotli --force --quality=11 --output="$file.br" "$file"
  gzip --force --best --keep --no-name "$file"
done
