#!/usr/bin/env bash
# PostToolUse: Write|Edit|MultiEdit
#
# After any Android strings.xml edit, diffs EN and FR string keys.
# Any key present in one file but not the other is a translation gap.

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)

[[ "$FILE" == */res/values*/strings.xml ]] || exit 0

EN="apps/android/app/src/main/res/values/strings.xml"
FR="apps/android/app/src/main/res/values-fr/strings.xml"

[[ -f "$EN" && -f "$FR" ]] || exit 0

EN_KEYS=$(grep -oE 'name="[^"]+"' "$EN" | cut -d'"' -f2 | sort)
FR_KEYS=$(grep -oE 'name="[^"]+"' "$FR" | cut -d'"' -f2 | sort)

MISSING_FR=$(comm -23 <(echo "$EN_KEYS") <(echo "$FR_KEYS"))
MISSING_EN=$(comm -13 <(echo "$EN_KEYS") <(echo "$FR_KEYS"))

if [[ -z "$MISSING_FR" && -z "$MISSING_EN" ]]; then
    COUNT=$(echo "$EN_KEYS" | wc -l | tr -d ' ')
    echo "Localization parity: OK ($COUNT strings, EN=FR)"
    exit 0
fi

echo "LOCALIZATION PARITY GAPS — translate the missing strings before finishing:"
[[ -n "$MISSING_FR" ]] && echo "  Missing from values-fr/strings.xml: $MISSING_FR"
[[ -n "$MISSING_EN" ]] && echo "  Missing from values/strings.xml:    $MISSING_EN"
exit 2
