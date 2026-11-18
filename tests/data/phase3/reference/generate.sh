#!/usr/bin/env bash
# Regenerates reference outputs in this folder from Structurizr 6.2.4:
# the CLI war (v2026.10.26, ~/.local/share/structurizr/structurizr.war) or
# the source build of structurizr-application (same structurizr-export
# 6.2.4), whichever STRUCTURIZR_WAR names. Run by hand (exporters row,
# 2026-11-18; `edge` added in fix round 1); tests never run Java.
# Usage: tests/data/phase3/reference/generate.sh [fixture...]
#   fixture: example, deployment, dynamic, edge, styles (default: all)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
data="$here/../.."
war="${STRUCTURIZR_WAR:-$HOME/.local/share/structurizr/structurizr.war}"
if [ ! -f "$war" ]; then
  war="$HOME/.local/src/structurizr/structurizr-application/target/structurizr-1.0.0.war"
fi
wanted="${*:-example deployment dynamic edge styles}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
(cd "$work" && unzip -qo "$war" 'WEB-INF/lib/*')
cp="$work/WEB-INF/lib/*"

# c4rizer-only view lines (layout and drawing aids) are removed so that
# Structurizr parses the fixture; they never change a text export.
strip() {
  grep -Ev '^[[:space:]]*(subtitle|technology|bus|labelWidth|maxCrossings|at|size|hide|focus|via|line)[[:space:]]' "$1"
}

for fixture in example:"$data/example.dsl" deployment:"$data/phase3/deployment.dsl" dynamic:"$data/phase3/dynamic.dsl" edge:"$data/phase3/edge.dsl"; do
  name="${fixture%%:*}"; src="${fixture#*:}"
  case " $wanted " in *" $name "*) ;; *) continue ;; esac
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

case " $wanted " in *" styles "*) ;; *) exit 0 ;; esac

# P3-421: the theme of the styles fixture. Its `theme`/`themes` lines go
# too: the theme export holds the workspace's own styles only, and
# `theme default` would make Structurizr fetch a URL.
mkdir -p "$here/styles"
strip "$data/styles/all_styles.dsl" | grep -Ev '^[[:space:]]*themes?[[:space:]]' > "$work/all_styles.dsl"
cp "$work/all_styles.dsl" "$here/styles/structurizr.dsl"
cp -r "$data/styles/icons" "$work/"
java -jar "$war" export -w "$work/all_styles.dsl" -f theme -o "$here/styles" > /dev/null
