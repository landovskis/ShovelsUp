#!/usr/bin/env bash
# PostToolUse: Write|Edit|MultiEdit
#
# Runs `cargo clippy` after any Rust source file in apps/web/ is modified.
# -D warnings promotes lint warnings to errors, matching CI.
# Stdout goes to Claude; exit 2 signals that Claude should act on the errors.

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ "$FILE" == *.rs ]] || exit 0
[[ "$FILE" == */apps/web/* ]] || exit 0

echo "Running cargo clippy..." >&2
OUTPUT=$(cargo clippy --manifest-path apps/web/Cargo.toml -- -D warnings 2>&1)
STATUS=$?

if [[ $STATUS -eq 0 ]]; then
    echo "cargo clippy: OK"
    exit 0
else
    echo "cargo clippy FAILED — fix these errors and warnings before continuing:"
    echo "$OUTPUT"
    exit 2
fi
