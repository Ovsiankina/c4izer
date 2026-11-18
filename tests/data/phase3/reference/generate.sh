#!/usr/bin/env bash
# Regenerates every reference output in this folder from the installed
# Structurizr 6.2.4 (CLI v2026.10.26, ~/.local/share/structurizr/structurizr.war).
# Run once by hand (exporters row, 2026-11-18); tests never run Java.
# Usage: tests/data/phase3/reference/generate.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
data="$here/../.."
war="${STRUCTURIZR_WAR:-$HOME/.local/share/structurizr/structurizr.war}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
(cd "$work" && unzip -qo "$war" 'WEB-INF/lib/*')
cp="$work/WEB-INF/lib/*"

# c4rizer-only view lines (layout and drawing aids) are removed so that
# Structurizr parses the fixture; they never change a text export.
strip() {
  grep -Ev '^[[:space:]]*(subtitle|technology|bus|labelWidth|maxCrossings|at|size|hide|focus|via|line)[[:space:]]' "$1"
}

for fixture in example:"$data/example.dsl" deployment:"$data/phase3/deployment.dsl" dynamic:"$data/phase3/dynamic.dsl"; do
  name="${fixture%%:*}"; src="${fixture#*:}"
  out="$here/$name"
  rm -rf "$out"; mkdir -p "$out"
  strip "$src" > "$work/$name.dsl"
  cp "$work/$name.dsl" "$out/structurizr.dsl"
  for format in plantuml/structurizr plantuml/c4plantuml mermaid websequencediagrams json; do
    dir="$out/${format//\//-}"
    java -jar "$war" export -w "$work/$name.dsl" -f "$format" -o "$dir" > /dev/null
  done
  java -cp "$cp" "$here/Probe.java" "$work/$name.dsl" "$out"
done

# P3-421: the theme of the styles fixture. Its `theme`/`themes` lines go
# too: the theme export holds the workspace's own styles only, and
# `theme default` would make Structurizr fetch a URL.
mkdir -p "$here/styles"
strip "$data/styles/all_styles.dsl" | grep -Ev '^[[:space:]]*themes?[[:space:]]' > "$work/all_styles.dsl"
cp "$work/all_styles.dsl" "$here/styles/structurizr.dsl"
cp -r "$data/styles/icons" "$work/"
java -jar "$war" export -w "$work/all_styles.dsl" -f theme -o "$here/styles" > /dev/null
