#!/usr/bin/env bash
# Whether known-vulnerability audits apply to one run of the `dependencies`
# workflow (#230). Prints "true" or "false", nothing else.
#
#   dependency-changes.sh <event> <base>
#
# A pull request answers for what it changes: it prints "false" only for a
# pull_request whose diff from <base> (its merge commit's first parent, the
# branch it merges into) touches no dependency at all. Any dependency
# change, backend or frontend, turns on both audits. Every other event, a
# missing base, or a diff that cannot be computed prints "true": it fails
# closed, never assuming "nothing changed".
set -u

event=${1:-}
base=${2:-}

dependencies='^backend/(.+/)?Cargo\.toml$|^backend/Cargo\.lock$|^backend/deny\.toml$'
dependencies="$dependencies|^frontend/package\.json$|^frontend/package-lock\.json$"
# A change to the rule itself is audited in full.
dependencies="$dependencies|^\.github/workflows/dependencies\.yml$|^\.github/scripts/dependency-changes\.sh$"

if [ "$event" != "pull_request" ] || [ -z "$base" ]; then
  echo true
  exit 0
fi
# Renames count as a deletion plus an addition: both paths are listed.
if ! changed=$(git diff --no-renames --name-only "$base" HEAD 2>/dev/null); then
  echo true
  exit 0
fi
if grep -qE "$dependencies" <<< "$changed"; then
  echo true
else
  echo false
fi
