#!/bin/bash
# Fails when a string catalog under gui/ lacks a finished translation: every key that is not stale and not
# marked "Don't Translate" must have, for each language in the project's knownRegions other than en (the
# source language) and Base, a localization whose every string unit, plural cases included, is
# `translated`. Adding a language to knownRegions makes this fail until the language is translated.
#
#     check-strings.sh
set -euo pipefail

gui="$(cd "$(dirname "$0")/.." && pwd)"
project="$gui/PhoneLink.xcodeproj/project.pbxproj"

languages=$(sed -n '/knownRegions = (/,/);/p' "$project" | sed -e '1d' -e '$d' | tr -d '\t ,"' | grep -vxE 'en|Base')
if [ -z "$languages" ]; then
    echo "error: no languages other than en in $project's knownRegions" >&2
    exit 1
fi

failed=0
while IFS= read -r catalog; do
    for lang in $languages; do
        missing=$(jq -r --arg lang "$lang" '
            .strings | to_entries[]
            | select(.value.extractionState != "stale" and .value.shouldTranslate != false)
            | select(
                (.value.localizations[$lang] // null) as $l
                | $l == null
                  or ([$l | .. | objects | select(has("stringUnit")) | .stringUnit.state] | length == 0 or any(. != "translated"))
              )
            | .key' "$catalog")
        if [ -n "$missing" ]; then
            echo "${catalog#"$gui"/}: not translated to $lang:" >&2
            sed 's/^/  /' <<<"$missing" >&2
            failed=1
        fi
    done
done < <(find "$gui" -name '*.xcstrings' -not -path '*/build/*')

if [ $failed -ne 0 ]; then
    exit 1
fi
echo "every catalog is translated to: $(echo $languages)"
