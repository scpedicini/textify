#!/usr/bin/env bash
set -euo pipefail
[[ $# -eq 1 && -d "$1" ]] || { echo "Usage: $0 EXISTING_DIRECTORY" >&2; exit 2; }
destination="$1"

for records in 8 1000; do
    name="TEST-short.json"
    [[ "$records" == 1000 ]] && name="TEST-10000-lines.json"
    awk -v records="$records" 'BEGIN {
        print "["
        for (i = 1; i <= records; i++) {
            print "  {"
            printf "    \"id\": %d,\n", i
            printf "    \"section\": \"Minimap test section %d\",\n", int((i - 1) / 20) + 1
            printf "    \"active\": %s,\n", (i % 3 ? "true" : "false")
            print "    \"settings\": {"
            print "      \"theme\": \"dark\","
            printf "      \"fontSize\": %d\n", 12 + i % 8
            print "    },"
            printf "    \"note\": \"%s\"\n", (i % 4 ? "Drag slowly, reverse direction, then reach either end." : "A shorter entry.")
            printf "  }%s\n", (i < records ? "," : "")
        }
        print "]"
    }' > "$destination/$name"
done

awk 'BEGIN {
    print "# Textify Dev: minimap feel test\n"
    print "These are generated test documents. Edit them freely.\n"
    for (i = 1; i <= 80; i++) {
        printf "## Section %d\n\n", i
        print "Try **slow drags**, *quick reversals*, and grabbing near an edge.\n"
        print "- Notice the indentation and gaps."
        print "- Drag outside the minimap while holding the button.\n"
        print "```json\n{"
        printf "  \"section\": %d,\n", i
        print "  \"enabled\": true\n}\n```\n"
        if (i % 3 == 0) print "### A nested heading\n\nExtra whitespace makes this section easy to recognize.\n"
    }
}' > "$destination/TEST-sections.md"

awk 'BEGIN {
    for (i = 1; i <= 50000; i++) {
        if (i % 100 == 1) printf "SECTION %03d — lines %d–%d\n", int((i - 1) / 100) + 1, i, i + 99
        else if (i % 100 > 92) print ""
        else printf "%*sLine %05d: %s\n", (i % 5) * 4, "", i, (i % 3 ? "Plain-text minimap navigation and indentation." : "Short row.")
    }
}' > "$destination/TEST-50000-lines.txt"
