#!/bin/sh
# check-schema-guard.sh — fail if db.rs schema surface changed without approval.
#
# Usage: check-schema-guard.sh [<base-ref> [<head-ref>]]
#   base-ref: commit to diff from  (default: origin/main)
#   head-ref: commit to diff to    (default: HEAD)
#
# Schema surface — any changed (+/-) line in src/db.rs that touches:
#   • CREATE_TABLES_SQL  (the canonical fresh-DB schema constant)
#   • fn migrate_*       (table-recreation migration functions)
#   • ALTER TABLE        (column-add migrations in the guarded loop)
#   • CREATE TABLE       (schema SQL inside migration batches)
#
# Approval marker — any commit message in the range that contains:
#   schema-change-approved: <ticket>
#
# To approve a schema change:
#   1. Get explicit owner sign-off (see CLAUDE.md § Schema-change policy).
#   2. Add "schema-change-approved: br-NNN" to the relevant commit message.
#   3. Push normally — this guard will pass.
#
# Bypass (emergency only, requires owner approval):
#   git push --no-verify

set -e

BASE="${1:-origin/main}"
HEAD="${2:-HEAD}"

# If base ref doesn't exist (e.g. fresh clone with no upstream yet), skip.
if ! git rev-parse --verify "$BASE" >/dev/null 2>&1; then
    echo "schema-guard: base ref '$BASE' not found — skipping schema check." >&2
    exit 0
fi

# Detect changed lines in the schema surface of src/db.rs.
# Match both additions (+) and deletions (-) but not diff headers (+++/---).
schema_diff=$(git diff "${BASE}..${HEAD}" -- src/db.rs 2>/dev/null \
    | grep -E '^[+-][^+-]' \
    | grep -E 'CREATE_TABLES_SQL|fn migrate_[a-z_]+|ALTER TABLE|CREATE TABLE' \
    || true)

if [ -z "$schema_diff" ]; then
    exit 0  # No schema surface touched — all clear.
fi

# Schema surface was touched. Check for approval marker in any commit message.
if git log "${BASE}..${HEAD}" --format='%B' 2>/dev/null \
        | grep -qi 'schema-change-approved:'; then
    exit 0  # Explicitly approved — allow push.
fi

echo "" >&2
echo "schema-guard: db.rs schema surface modified without approval." >&2
echo "" >&2
echo "  Changed lines:" >&2
printf '%s\n' "$schema_diff" | sed 's/^/    /' >&2
echo "" >&2
echo "  Required action (choose one):" >&2
echo "    a) If this change was NOT in the original spec:" >&2
echo "       Stop. Revert the schema change. Open a follow-up ticket" >&2
echo "       for owner approval before implementing." >&2
echo "    b) If you have explicit owner approval:" >&2
echo "       Add 'schema-change-approved: br-NNN' to a commit message" >&2
echo "       in this branch, then push again." >&2
echo "" >&2
echo "  See CLAUDE.md § Schema-change policy." >&2
echo "" >&2
exit 1
