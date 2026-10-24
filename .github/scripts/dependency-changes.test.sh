#!/usr/bin/env bash
# Reproducible checks of dependency-changes.sh (#230) on a throwaway git
# repository: every pull request is a real merge commit into its base, as
# GitHub checks it out, so the diff covers all of its commits.
set -euo pipefail

rule="$(cd "$(dirname "$0")" && pwd)/dependency-changes.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q -b main
git config user.email test@invalid
git config user.name test
git config core.autocrlf false
mkdir -p backend/crates/core/src frontend/src docs
for file in backend/Cargo.toml backend/Cargo.lock backend/deny.toml \
  backend/crates/core/Cargo.toml backend/crates/core/src/lib.rs \
  frontend/package.json frontend/package-lock.json frontend/src/app.ts docs/a.md; do
  echo base > "$file"
done
git add -A
git commit -q -m base

failures=0
# Runs `change` on a branch with `commits` commits, merges it into main
# without fast-forward, and expects `rule` to answer `expected` for the
# `area` job (a label: both jobs share one rule).
check() {
  local name=$1 area=$2 expected=$3
  shift 3
  git checkout -q -b "pr-$name-$area" main
  "$@"
  git checkout -q main
  git checkout -q -b "merge-$name-$area" main
  git merge -q --no-ff --no-edit "pr-$name-$area"
  local got
  got=$(bash "$rule" pull_request HEAD^1)
  git checkout -q main
  if [ "$got" = "$expected" ]; then
    echo "ok   $name ($area): $got"
  else
    echo "FAIL $name ($area): expected $expected, got $got"
    failures=$((failures + 1))
  fi
}
edit() { echo "$RANDOM" >> "$1"; git add -A; git commit -q -m "edit $1"; }
code_only() { edit backend/crates/core/src/lib.rs; edit frontend/src/app.ts; edit docs/a.md; }
add_manifest() { mkdir -p backend/crates/new; echo new > backend/crates/new/Cargo.toml; git add -A; git commit -q -m add; }
delete_manifest() { git rm -q backend/crates/core/Cargo.toml; git commit -q -m delete; }
rename_manifest() { git mv backend/crates/core/Cargo.toml backend/crates/core/Cargo.toml.old; git commit -q -m rename; }
earlier_commit() { edit backend/Cargo.lock; edit backend/crates/core/src/lib.rs; }

check code-only backend false code_only
check code-only frontend false code_only
check lockfile backend true edit backend/Cargo.lock
check workspace-manifest backend true edit backend/Cargo.toml
check nested-manifest backend true edit backend/crates/core/Cargo.toml
check new-manifest backend true add_manifest
check deleted-manifest backend true delete_manifest
check renamed-manifest backend true rename_manifest
check deny-policy backend true edit backend/deny.toml
check earlier-commit backend true earlier_commit
check package-json frontend true edit frontend/package.json
check package-lock frontend true edit frontend/package-lock.json
check package-lock-turns-on-backend backend true edit frontend/package-lock.json
check lockfile-turns-on-frontend frontend true edit backend/Cargo.lock
check package-json-turns-on-backend backend true edit frontend/package.json
check deny-policy-turns-on-frontend frontend true edit backend/deny.toml

# Every event other than a pull request audits in full, with no change.
for event in push schedule workflow_dispatch; do
  for area in backend frontend; do
    got=$(bash "$rule" "$event" HEAD^1)
    if [ "$got" = true ]; then echo "ok   $event ($area): true"; else
      echo "FAIL $event ($area): expected true, got $got"; failures=$((failures + 1)); fi
  done
done
# Fails closed: a base that cannot be diffed, no base, or an unknown area.
for args in "pull_request no-such-commit" "pull_request" "" ; do
  # shellcheck disable=SC2086
  got=$(bash "$rule" $args)
  if [ "$got" = true ]; then echo "ok   fails closed ($args): true"; else
    echo "FAIL fails closed ($args): expected true, got $got"; failures=$((failures + 1)); fi
done

# The workflow's own step, as written in dependencies.yml, fails closed: a
# missing detector or an answer other than true/false fails the job, and
# never writes a value that would skip an audit.
workflow="$(cd "$(dirname "$rule")/../workflows" && pwd)/dependencies.yml"
python=
for candidate in python3 python; do
  if "$candidate" -c '' 2>/dev/null; then python=$candidate; break; fi
done
step=$("$python" - "$workflow" <<'PY'
import re, sys
text = open(sys.argv[1], encoding='utf-8').read()
match = re.search(r"\n        run: \|\n((?:          .*\n)+?)(?=      - )", text[text.index('Do known vulnerabilities apply'):])
print('\n'.join(line[10:] for line in match.group(1).splitlines()))
PY
)
step_run() {
  # $1: directory holding .github/scripts/dependency-changes.sh (or not).
  local output
  output=$(mktemp)
  (cd "$1" && EVENT=pull_request GITHUB_OUTPUT="$output" bash -c "$step") >/dev/null 2>&1
  local status=$?
  echo "$status $(cat "$output")"
  rm -f "$output"
}
fake=$(mktemp -d)
mkdir -p "$fake/.github/scripts"
got=$(step_run "$fake")
case "$got" in
  "0 "*) echo "FAIL step without a detector: $got"; failures=$((failures + 1)) ;;
  *) echo "ok   step without a detector fails: ${got%% *}" ;;
esac
printf '#!/usr/bin/env bash\necho maybe\n' > "$fake/.github/scripts/dependency-changes.sh"
got=$(step_run "$fake")
case "$got" in
  "0 "* | *advisories=*) echo "FAIL step with an invalid answer: $got"; failures=$((failures + 1)) ;;
  *) echo "ok   step with an invalid answer fails: ${got%% *}" ;;
esac
printf '#!/usr/bin/env bash\necho false\n' > "$fake/.github/scripts/dependency-changes.sh"
got=$(step_run "$fake")
if [ "$got" = "0 advisories=false" ]; then echo "ok   step with a valid answer: $got"; else
  echo "FAIL step with a valid answer: $got"; failures=$((failures + 1)); fi
rm -rf "$fake"

if [ "$failures" -ne 0 ]; then
  echo "$failures failing case(s)"
  exit 1
fi
echo "all cases pass"
