#!/usr/bin/env bash
# PostToolUse: Write|Edit|MultiEdit
#
# Runs xcodebuild after any Swift file in apps/ios/ is modified.
# First build populates DerivedData; subsequent incremental builds are fast.
# CODE_SIGNING_ALLOWED=NO avoids needing a development certificate in CI/hooks.

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ "$FILE" == *.swift ]] || exit 0
[[ "$FILE" == */apps/ios/* ]] || exit 0

echo "Building iOS app (xcodebuild, incremental)..." >&2
OUTPUT=$(xcodebuild build \
  -project apps/ios/ShovelsUp.xcodeproj \
  -scheme ShovelsUp \
  -sdk iphonesimulator \
  CODE_SIGNING_ALLOWED=NO \
  -quiet 2>&1)
STATUS=$?

if [[ $STATUS -eq 0 ]]; then
    echo "xcodebuild: OK"
    exit 0
else
    echo "xcodebuild FAILED — fix these errors before continuing:"
    echo "$OUTPUT"
    exit 2
fi
