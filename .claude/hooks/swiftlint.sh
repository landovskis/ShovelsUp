#!/usr/bin/env bash
# PostToolUse: Write|Edit|MultiEdit
#
# Runs swiftlint on the edited Swift file if swiftlint is installed.
# Skips silently when swiftlint is absent — no hard dependency.

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ "$FILE" == *.swift ]] || exit 0
[[ "$FILE" == */apps/ios/* ]] || exit 0
[[ -f "$FILE" ]] || exit 0

command -v swiftlint &>/dev/null || exit 0

OUTPUT=$(swiftlint lint --quiet --path "$FILE" 2>&1)
STATUS=$?

if [[ $STATUS -eq 0 && -z "$OUTPUT" ]]; then
    echo "swiftlint: OK"
    exit 0
else
    echo "swiftlint issues in $(basename "$FILE"):"
    echo "$OUTPUT"
    # Exit 2 only for errors; warnings are informational
    if echo "$OUTPUT" | grep -q " error:"; then
        exit 2
    fi
fi
