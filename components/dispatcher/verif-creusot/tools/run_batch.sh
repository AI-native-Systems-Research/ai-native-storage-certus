#!/bin/bash
# Run `cargo creusot <module>` (2 parallel provers) for each module listed in $1 not yet in $2,
# appending "<module> <PROVED|FAILED|TIMEOUT> <secs>" to $2; stops after ~520 s wall.
export PATH="$HOME/.local/share/creusot/bin:$PATH"
list=$1; res=$2; start=$(date +%s)
touch "$res"
while read -r m; do
  [ -z "$m" ] && continue
  grep -q "^$m " "$res" && continue
  now=$(date +%s); [ $((now-start)) -gt 520 ] && break
  t0=$(date +%s)
  mm=$(echo "$m" | sed "s/__mutant$/_mutant/")
  out=$(timeout 300 cargo creusot "$mm" --why3find-arg=-j --why3find-arg=2 2>&1 | tail -1); rc=$?
  t1=$(date +%s)
  if echo "$out" | grep -q "Proved"; then st=PROVED; elif [ $rc -eq 124 ] || echo "$out" | grep -q Terminated; then st=TIMEOUT; else st=FAILED; fi
  echo "$m $st $((t1-t0))" >> "$res"
done < "$list"
echo "done: $(wc -l < "$res") / $(grep -c . "$list")"
