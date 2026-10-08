#!/usr/bin/env bash
# Publishes the end of a failed job's output on a `ci-log-<job>` branch,
# so it can be read without signing in to GitHub.
set -eu
name="$1"; token="$2"
log="$RUNNER_TEMP/ci.log"
[ -f "$log" ] || exit 0
work="$RUNNER_TEMP/ci-log-$name"
rm -rf "$work" && mkdir -p "$work" && cd "$work"
git init -q -b "ci-log-$name"
tail -n 500 "$log" > "$name.log"
git add .
git -c user.name="ci" -c user.email="ci@users.noreply.github.com" commit -q -m "log of $GITHUB_SHA"
git push -q -f "https://x-access-token:$token@github.com/$GITHUB_REPOSITORY" "ci-log-$name"
