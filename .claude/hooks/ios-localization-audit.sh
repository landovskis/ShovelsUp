#!/usr/bin/env bash
# PostToolUse: Write|Edit|MultiEdit
#
# After any edit to Localizable.xcstrings, checks that every string key has
# both "en" and "fr" localizations with state "translated".
# xcstrings is JSON, so jq does the heavy lifting.

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ "$FILE" == *.xcstrings ]] || exit 0
[[ -f "$FILE" ]] || exit 0

check_lang() {
    local lang="$1"
    jq -r --arg lang "$lang" '
        .strings
        | to_entries[]
        | select(
            (.value.localizations[$lang] == null) or
            (.value.localizations[$lang].stringUnit.state != "translated")
          )
        | .key
    ' "$FILE"
}

MISSING_FR=$(check_lang "fr")
MISSING_EN=$(check_lang "en")

if [[ -z "$MISSING_FR" && -z "$MISSING_EN" ]]; then
    COUNT=$(jq '.strings | length' "$FILE")
    echo "iOS localization parity: OK ($COUNT strings, en=fr)"
    exit 0
fi

echo "iOS LOCALIZATION GAPS in $(basename "$FILE") — add missing translations before finishing:"
if [[ -n "$MISSING_FR" ]]; then
    echo "  Missing/untranslated FR:"
    echo "$MISSING_FR" | sed 's/^/    - /'
fi
if [[ -n "$MISSING_EN" ]]; then
    echo "  Missing/untranslated EN:"
    echo "$MISSING_EN" | sed 's/^/    - /'
fi
exit 2
