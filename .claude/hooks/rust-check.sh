#!/usr/bin/env bash
# PostToolUse: Write|Edit|MultiEdit
#
# Runs `cargo check` after any Rust source file in apps/web/ is modified.
# Stdout goes to Claude; exit 2 signals that Claude should act on the errors.

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ "$FILE" == *.rs ]] || exit 0
[[ "$FILE" == */apps/web/* ]] || exit 0

echo "Running cargo check..." >&2
OUTPUT=$(cargo check --manifest-path apps/web/Cargo.toml 2>&1)
STATUS=$?

if [[ $STATUS -eq 0 ]]; then
    echo "cargo check: OK"
    exit 0
else
    echo "cargo check FAILED — fix these errors before continuing:"
    echo "$OUTPUT"
    exit 2
fi
