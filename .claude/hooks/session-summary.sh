#!/usr/bin/env bash
# Stop hook — runs at the end of every Claude turn.
#
# Prints a compact session-diff summary and a cross-app localization parity
# report to stderr so it appears in the terminal without injecting noise into
# Claude's context.

{
    # --- git diff summary ---
    UNSTAGED=$(git diff --stat 2>/dev/null)
    STAGED=$(git diff --cached --stat 2>/dev/null)

    if [[ -n "$STAGED" || -n "$UNSTAGED" ]]; then
        echo "=== Session diff ==="
        [[ -n "$STAGED"   ]] && printf "Staged:\n%s\n" "$STAGED"
        [[ -n "$UNSTAGED" ]] && printf "Unstaged:\n%s\n" "$UNSTAGED"
    fi

    # --- Android EN/FR parity ---
    EN="apps/android/app/src/main/res/values/strings.xml"
    FR="apps/android/app/src/main/res/values-fr/strings.xml"

    if [[ -f "$EN" && -f "$FR" ]]; then
        EN_KEYS=$(grep -oP 'name="\K[^"]+' "$EN" | sort)
        FR_KEYS=$(grep -oP 'name="\K[^"]+' "$FR" | sort)
        MISSING_FR=$(comm -23 <(echo "$EN_KEYS") <(echo "$FR_KEYS"))
        MISSING_EN=$(comm -13 <(echo "$EN_KEYS") <(echo "$FR_KEYS"))

        if [[ -n "$MISSING_FR" || -n "$MISSING_EN" ]]; then
            echo ""
            echo "=== Android localization gaps ==="
            [[ -n "$MISSING_FR" ]] && echo "  Missing FR: $MISSING_FR"
            [[ -n "$MISSING_EN" ]] && echo "  Missing EN: $MISSING_EN"
        fi
    fi

    # --- iOS parity (xcstrings has both languages; warn if file is absent) ---
    XCSTRINGS="apps/ios/ShovelsUp/Localizable.xcstrings"
    if [[ ! -f "$XCSTRINGS" ]]; then
        echo ""
        echo "=== iOS localization ==="
        echo "  WARNING: Localizable.xcstrings not found at expected path: $XCSTRINGS"
    fi

} >&2

exit 0
