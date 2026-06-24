#!/usr/bin/env bash
# PreToolUse: Write|Edit|MultiEdit
#
# Injects a bilingual reminder into Claude's context whenever a localization
# file is about to be written (Android strings.xml or iOS xcstrings).
# Non-blocking (exit 0).

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ -z "$FILE" ]] && exit 0

if [[ "$FILE" == */res/values/strings.xml ]]; then
    FR="${FILE/res\/values\/strings.xml/res\/values-fr\/strings.xml}"
    echo "LOCALIZATION: Editing EN (default) strings. Ensure the FR counterpart is also updated: $FR"
fi

if [[ "$FILE" == */res/values-fr/strings.xml ]]; then
    EN="${FILE/res\/values-fr\/strings.xml/res\/values\/strings.xml}"
    echo "LOCALIZATION: Editing FR strings. Ensure the EN counterpart is also updated: $EN"
fi

# iOS: xcstrings holds both languages in one file — remind Claude that every
# key needs both "en" and "fr" entries with state "translated".
if [[ "$FILE" == *.xcstrings ]]; then
    echo "LOCALIZATION: Editing iOS xcstrings. Every string key must have both \"en\" and \"fr\" localizations with state \"translated\"."
fi

exit 0
