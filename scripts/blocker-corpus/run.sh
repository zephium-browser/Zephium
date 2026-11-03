#!/bin/zsh
# One release pass for crates/zephium-blocker/src/webkit_pruned.txt: records the page corpus with
# no blocking, replays it through WebKit with every rule of the bundled lists reporting its
# matches, and writes the wildcard block rules nothing matched. See docs/adblock.md.
# usage: scripts/blocker-corpus/run.sh OUT_DIR
set -euo pipefail
here=${0:A:h}
repo=${here:h:h}
out=${1:?usage: run.sh OUT_DIR}
mkdir -p "$out"
out=${out:A}

for tool in record replay; do
  xcrun swiftc -parse-as-library -O -warnings-as-errors -module-cache-path "$out/swift-cache" \
    -framework WebKit "$here/$tool.swift" -o "$out/$tool"
done
{
  print '(async () => {'
  cat "$here/consent-youtube.js"
  sed -e 's/__RECIPE__/null/' -e "s/__MODE__/'act'/" -e 's/__WAIT__/4000/' \
    "$repo/xtask/src/blocker_bench/consent.js"
  print '})().catch(() => {});'
} > "$out/consent.js"

cd "$repo"
rm -f "$out/rules.json"
cargo xtask blocker-prune materialize --output "$out/rules.json"
cargo xtask blocker-prune notify --rules "$out/rules.json" --output "$out/notify.json"
cargo xtask blocker-prune hosts --rules "$out/rules.json" --output "$out/rulehosts.txt"

pages=0
corpora=() visits=()
for list in "$here"/pages/*.txt "$out/rulehosts.txt"; do
  name=${list:t:r}
  echo "recording $name ($(wc -l < "$list" | tr -d ' ') pages)"
  "$out/record" "$list" "$out/corpus-$name.jsonl" "$out/visits-$name.jsonl" 5 "$out/consent.js" \
    2> "$out/record-$name.log"
  corpora+=("$out/corpus-$name.jsonl") visits+=("$out/visits-$name.jsonl")
  pages=$(( pages + $(grep -c . "$list") ))
done
cat "${corpora[@]}" > "$out/corpus.jsonl"
echo "replaying $(wc -l < "$out/corpus.jsonl" | tr -d ' ') loads"
"$out/replay" "$out/corpus.jsonl" "$out/matches.jsonl" "$out/rules.json" "$out/notify.json" \
  2> "$out/replay.log"
cargo xtask blocker-prune build --rules "$out/rules.json" --loads "$out/corpus.jsonl" \
  --matches "$out/matches.jsonl" --visits "${visits[@]}" \
  --corpus "recorded $(date +%F) on $pages pages" \
  --output crates/zephium-blocker/src/webkit_pruned.txt
