#!/usr/bin/env bash
# Complexity metrics for a Rust codebase.
# Usage: complexity.sh [target_dir]

set -uo pipefail

TARGET="${1:-.}"

echo ""
echo "═══════════════════════════════════════════════════════════"
echo " COMPLEXITY METRICS"
echo "═══════════════════════════════════════════════════════════"

# --- Per-component SLOC breakdown ---
echo ""
echo "── Lines of Rust code per component ──"
echo ""
printf "%-50s %8s %8s %8s\n" "Component" "Code" "Tests" "Total"
printf "%-50s %8s %8s %8s\n" "─────────" "────" "─────" "─────"

find "$TARGET" -name "*.rs" \
    -not -path "*/target/*" \
    -not -path "*/deps/spdk-build/*" \
    -print | sort | awk -v target="$TARGET" '
{
    file = $0
    # Determine component from path
    rel = file
    sub("^" target "/", "", rel)
    n = split(rel, parts, "/")
    if (n >= 3) component = parts[1] "/" parts[2] "/" parts[3]
    else if (n >= 2) component = parts[1] "/" parts[2]
    else component = parts[1]

    code = 0; test = 0; in_test = 0
    while ((getline line < file) > 0) {
        gsub(/^[[:space:]]+/, "", line)
        if (line == "") continue
        if (line ~ /^\/\//) continue
        if (line ~ /#\[cfg\(test\)\]/) { in_test = 1 }
        if (line ~ /^#\[test\]/) { in_test = 1 }
        if (in_test) { test++ } else { code++ }
    }
    close(file)
    comp_code[component] += code
    comp_test[component] += test
}
END {
    for (c in comp_code) {
        total = comp_code[c] + comp_test[c]
        printf "%-50s %8d %8d %8d\n", c, comp_code[c], comp_test[c], total
    }
}' | sort -k2 -rn

# --- Largest files ---
echo ""
echo "── Top 15 largest source files (non-test, by code lines) ──"
echo ""
printf "%-70s %8s\n" "File" "Lines"
printf "%-70s %8s\n" "────" "─────"

find "$TARGET" -name "*.rs" \
    -not -path "*/target/*" \
    -not -path "*/deps/spdk-build/*" \
    -not -name "*test*" \
    -print0 | xargs -0 wc -l 2>/dev/null | sort -rn | awk -v target="$TARGET" '
NR <= 16 && NR > 1 {
    rel = $2
    sub("^" target "/", "", rel)
    printf "%-70s %8d\n", rel, $1
}'

# --- Function count per file ---
echo ""
echo "── Top 15 files by function/method count ──"
echo ""
printf "%-70s %8s\n" "File" "Fns"
printf "%-70s %8s\n" "────" "───"

find "$TARGET" -name "*.rs" \
    -not -path "*/target/*" \
    -not -path "*/deps/spdk-build/*" \
    -print0 | xargs -0 grep -cE '^\s*(pub\s+)?(async\s+)?fn\s' 2>/dev/null | \
    awk -F: -v target="$TARGET" '{
        rel = $1; sub("^" target "/", "", rel)
        print $2, rel
    }' | sort -rn | head -15 | awk '{printf "%-70s %8d\n", $2, $1}'

# --- Unsafe blocks ---
echo ""
echo "── Unsafe usage ──"
echo ""
unsafe_count=$(find "$TARGET" -name "*.rs" \
    -not -path "*/target/*" \
    -not -path "*/deps/spdk-build/*" \
    -print0 | xargs -0 grep -l "unsafe" 2>/dev/null | wc -l)
unsafe_blocks=$(find "$TARGET" -name "*.rs" \
    -not -path "*/target/*" \
    -not -path "*/deps/spdk-build/*" \
    -print0 | xargs -0 grep -c "unsafe" 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
echo "Files containing unsafe:  $unsafe_count"
echo "Total unsafe occurrences: $unsafe_blocks"

# --- Deepest nesting ---
echo ""
echo "── Deepest nesting (max brace depth per file, top 10) ──"
echo ""
printf "%-70s %8s\n" "File" "Depth"
printf "%-70s %8s\n" "────" "─────"

find "$TARGET" -name "*.rs" \
    -not -path "*/target/*" \
    -not -path "*/deps/spdk-build/*" \
    -print0 | xargs -0 -I{} awk '
BEGIN { max=0; cur=0 }
{
    for (i=1; i<=length($0); i++) {
        c = substr($0,i,1)
        if (c == "{") { cur++; if (cur>max) max=cur }
        else if (c == "}") { if (cur>0) cur-- }
    }
}
END { print max, FILENAME }
' {} 2>/dev/null | awk -v target="$TARGET" '{
    rel = $2; sub("^" target "/", "", rel)
    print $1, rel
}' | sort -rn | head -10 | awk '{printf "%-70s %8d\n", $2, $1}'

echo ""
echo "═══════════════════════════════════════════════════════════"
