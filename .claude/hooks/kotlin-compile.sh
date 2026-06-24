#!/usr/bin/env bash
# PostToolUse: Write|Edit|MultiEdit
#
# Runs `./gradlew compileDebugKotlin` after any Kotlin file in apps/android/
# is modified.  Uses --quiet so only errors appear.  The Gradle daemon makes
# incremental runs fast after the first.

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ "$FILE" == *.kt ]] || exit 0
[[ "$FILE" == */apps/android/* ]] || exit 0

echo "Compiling Kotlin (incremental)..." >&2
OUTPUT=$(cd apps/android && ./gradlew compileDebugKotlin --quiet 2>&1)
STATUS=$?

if [[ $STATUS -eq 0 ]]; then
    echo "Kotlin compile: OK"
    exit 0
else
    echo "Kotlin compile FAILED — fix these errors before continuing:"
    echo "$OUTPUT"
    exit 2
fi
