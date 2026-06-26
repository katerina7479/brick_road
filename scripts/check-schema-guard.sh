#!/bin/sh
# check-schema-guard.sh — fail if db.rs schema surface changed without approval.
#
# Usage: check-schema-guard.sh [<base-ref> [<head-ref>]]
#   base-ref: commit to diff from  (default: origin/main)
#   head-ref: commit to diff to    (default: HEAD)
#
# Schema surface — two detection paths, both must be clear:
#
#   1. CREATE_TABLES_SQL body: the entire constant block is extracted from
#      both base and head versions of src/db.rs and compared directly.
#      This catches column additions/removals inside existing CREATE TABLE
#      blocks, which line-level grep cannot detect (the CREATE TABLE header
#      line is unchanged context in the diff; only the new column line has +).
#
#   2. Migration surface: +/- lines in src/db.rs that match:
#        fn migrate_*       table-recreation migration functions
#        ALTER TABLE        column-add migrations in the guarded loop
#        DROP TABLE         table removal (including in migrations)
#        CREATE TABLE       new tables added inside migration functions
#        CREATE INDEX / CREATE UNIQUE INDEX   partial index changes
#
# Approval marker — any commit message in the pushed range that contains:
#   schema-change-approved: <ticket>
#
# To approve a schema change:
#   1. Get explicit owner sign-off (see CLAUDE.md § Schema-change policy).
#   2. Add "schema-change-approved: br-NNN" to the relevant commit message.
#   3. Push normally — this guard will pass.
#
# Bypass (emergency only, requires owner approval):
#   git push --no-verify
#
# Self-test (verifies detection logic with a temp git repo):
#   scripts/check-schema-guard.sh --self-test

set -e

# ── Self-test mode ─────────────────────────────────────────────────────────────
if [ "${1-}" = "--self-test" ]; then
    PASS=0
    FAIL=0
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' EXIT

    GUARD="$(cd "$(dirname "$0")" && pwd)/check-schema-guard.sh"

    # Minimal db.rs with a CREATE_TABLES_SQL block.
    base_db='use rusqlite::Connection;

const CREATE_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS things (
    id   INTEGER PRIMARY KEY,
    name TEXT    NOT NULL
);
";

pub fn create_tables(conn: &Connection) {}
'
    # Same file with a new column added inside the CREATE TABLE block.
    col_added_db='use rusqlite::Connection;

const CREATE_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS things (
    id    INTEGER PRIMARY KEY,
    name  TEXT    NOT NULL,
    extra TEXT
);
";

pub fn create_tables(conn: &Connection) {}
'
    # Same file with a migrate_* function added (no column change).
    migrate_added_db='use rusqlite::Connection;

const CREATE_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS things (
    id   INTEGER PRIMARY KEY,
    name TEXT    NOT NULL
);
";

fn migrate_things_add_extra(conn: &Connection) {}
pub fn create_tables(conn: &Connection) {}
'

    # Helper: init a test repo, make base commit.
    setup_repo() {
        repo="$tmpdir/$1"
        mkdir -p "$repo/src"
        cd "$repo"
        git init -q
        git config user.email "test@test"
        git config user.name "Test"
        printf '%s' "$base_db" > src/db.rs
        git add src/db.rs
        git commit -q -m "base"
    }

    # ── Test 1: column added inside CREATE TABLE block — must FAIL ─────────────
    setup_repo t1
    printf '%s' "$col_added_db" > src/db.rs
    git add src/db.rs
    git commit -q -m "[t1] add extra column"
    if "$GUARD" HEAD~1 HEAD >/dev/null 2>&1; then
        echo "FAIL t1: column addition inside CREATE TABLE was not detected" >&2
        FAIL=$((FAIL+1))
    else
        echo "PASS t1: column addition detected"
        PASS=$((PASS+1))
    fi
    cd - >/dev/null

    # ── Test 2: migration fn added — must FAIL ─────────────────────────────────
    setup_repo t2
    printf '%s' "$migrate_added_db" > src/db.rs
    git add src/db.rs
    git commit -q -m "[t2] add migration"
    if "$GUARD" HEAD~1 HEAD >/dev/null 2>&1; then
        echo "FAIL t2: migrate_* addition was not detected" >&2
        FAIL=$((FAIL+1))
    else
        echo "PASS t2: migrate_* addition detected"
        PASS=$((PASS+1))
    fi
    cd - >/dev/null

    # ── Test 3: column added + approval marker — must PASS ────────────────────
    setup_repo t3
    printf '%s' "$col_added_db" > src/db.rs
    git add src/db.rs
    git commit -q -m "[t3] add extra column
schema-change-approved: br-999"
    if "$GUARD" HEAD~1 HEAD >/dev/null 2>&1; then
        echo "PASS t3: approval marker bypasses guard"
        PASS=$((PASS+1))
    else
        echo "FAIL t3: approval marker was not recognised" >&2
        FAIL=$((FAIL+1))
    fi
    cd - >/dev/null

    # ── Test 4: no schema change (edit outside CREATE_TABLES_SQL) — must PASS ──
    setup_repo t4
    # Append a Rust comment to the function body — outside the SQL constant.
    printf '\n// non-schema comment\n' >> src/db.rs
    git add src/db.rs
    git commit -q -m "[t4] non-schema comment change"
    if "$GUARD" HEAD~1 HEAD >/dev/null 2>&1; then
        echo "PASS t4: non-schema change passes guard"
        PASS=$((PASS+1))
    else
        echo "FAIL t4: guard fired on a non-schema change" >&2
        FAIL=$((FAIL+1))
    fi
    cd - >/dev/null

    echo ""
    echo "Self-test: $PASS passed, $FAIL failed"
    [ "$FAIL" -eq 0 ]
    exit $?
fi

# ── Normal guard mode ──────────────────────────────────────────────────────────

BASE="${1:-origin/main}"
HEAD="${2:-HEAD}"

# If base ref doesn't exist (e.g. fresh clone with no upstream yet), skip.
if ! git rev-parse --verify "$BASE" >/dev/null 2>&1; then
    echo "schema-guard: base ref '$BASE' not found — skipping schema check." >&2
    exit 0
fi

schema_changed=""

# ── Detection path 1: CREATE_TABLES_SQL body ──────────────────────────────────
# Extract the constant block from old and new versions of db.rs and compare
# directly. This is the only reliable way to detect column additions inside
# an existing CREATE TABLE block: the header line is unchanged context in the
# diff, so a line grep will always miss it.
if git cat-file -e "${BASE}:src/db.rs" 2>/dev/null \
        && git cat-file -e "${HEAD}:src/db.rs" 2>/dev/null; then
    old_sql=$(git show "${BASE}:src/db.rs" \
        | sed -n '/^const CREATE_TABLES_SQL/,/^";$/p')
    new_sql=$(git show "${HEAD}:src/db.rs" \
        | sed -n '/^const CREATE_TABLES_SQL/,/^";$/p')
    if [ "$old_sql" != "$new_sql" ]; then
        schema_changed="${schema_changed}  • CREATE_TABLES_SQL body changed\n"
    fi
fi

# ── Detection path 2: migration surface ───────────────────────────────────────
# +/- lines in src/db.rs matching migrate_* fns, ALTER/DROP TABLE,
# new CREATE TABLE (inside migration fns), CREATE [UNIQUE] INDEX.
mig_diff=$(git diff "${BASE}..${HEAD}" -- src/db.rs 2>/dev/null \
    | grep -E '^[+-][^+-]' \
    | grep -E 'fn migrate_[a-z_]+|ALTER TABLE|DROP TABLE|CREATE TABLE|CREATE UNIQUE INDEX|CREATE INDEX' \
    || true)

if [ -n "$mig_diff" ]; then
    schema_changed="${schema_changed}  • Migration surface changed:\n"
    schema_changed="${schema_changed}$(printf '%s' "$mig_diff" | sed 's/^/      /')\n"
fi

if [ -z "$schema_changed" ]; then
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
printf '%b' "$schema_changed" >&2
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
