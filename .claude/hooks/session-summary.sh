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

    # --- iOS EN/FR parity (xcstrings JSON) ---
    XCSTRINGS="apps/ios/ShovelsUp/Localizable.xcstrings"
    if [[ -f "$XCSTRINGS" ]]; then
        check_xcstrings_lang() {
            local lang="$1"
            jq -r --arg lang "$lang" '
                .strings | to_entries[]
                | select(
                    (.value.localizations[$lang] == null) or
                    (.value.localizations[$lang].stringUnit.state != "translated")
                  )
                | .key
            ' "$XCSTRINGS"
        }
        IOS_MISSING_FR=$(check_xcstrings_lang "fr")
        IOS_MISSING_EN=$(check_xcstrings_lang "en")
        if [[ -n "$IOS_MISSING_FR" || -n "$IOS_MISSING_EN" ]]; then
            echo ""
            echo "=== iOS localization gaps ==="
            [[ -n "$IOS_MISSING_FR" ]] && echo "$IOS_MISSING_FR" | sed 's/^/  Missing FR: /'
            [[ -n "$IOS_MISSING_EN" ]] && echo "$IOS_MISSING_EN" | sed 's/^/  Missing EN: /'
        fi
    else
        echo ""
        echo "=== iOS localization ==="
        echo "  WARNING: Localizable.xcstrings not found: $XCSTRINGS"
    fi

} >&2

exit 0
