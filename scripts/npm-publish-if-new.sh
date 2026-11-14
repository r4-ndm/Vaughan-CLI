#!/usr/bin/env bash
# Publish the npm package in <dir> unless that exact name@version is already on the registry.
# Makes a re-run of a half-failed release safe: already-published packages are skipped, never
# republished (npm refuses a republish, which used to wedge the whole job). Any other failure
# still fails the step.
set -euo pipefail
dir="${1:?usage: npm-publish-if-new.sh <package-dir>}"
name="$(node -p "require('./${dir}/package.json').name")"
version="$(node -p "require('./${dir}/package.json').version")"
code="$(curl -s -o /dev/null -w '%{http_code}' "https://registry.npmjs.org/${name}/${version}?nocache=$$")"
if [ "$code" = "200" ]; then
  echo "SKIP: ${name}@${version} is already on the registry"
  exit 0
fi
echo "PUBLISH: ${name}@${version} (registry answered ${code})"
(cd "$dir" && npm publish --provenance --access public)
