#!/usr/bin/env bash
# PreToolUse: Write|Edit|MultiEdit
#
# Injects a bilingual reminder into Claude's context whenever an Android
# string resource file is about to be written.  Non-blocking (exit 0).

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

exit 0
