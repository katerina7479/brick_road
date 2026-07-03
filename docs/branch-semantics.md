# Branch / ghost semantics ("rocks in the stream")

The single trickiest part of the model. Read this before touching plan
membership (`root_blocks`), and especially before implementing anything like
block splitting (#314). Code: `src/model.rs` — `fork_main`,
`link_main_block_to_branches`, `remove_block_from_plan`,
`accept_plan_as_main`, `rebase_siblings_onto_main`,
`prune_event_ghosts_from_branches`.

## The core idea

There is one **main** plan (`branch_start_day: None`, lowest id) and any
number of **branches** forked from it at a working day `F`
(`branch_start_day: Some(F)`). Blocks are **shared by id**: a branch does not
copy a block, it lists the same `WorkBlockId` in its own `root_blocks`. A
shared block seen from a branch is called a **ghost**; a block listed only in
the branch is **owned** by it.

Consequences:
- **Structure is global.** A block's name, dates, duration, color, children
  (`parent` links) live on the one shared `WorkBlock`. Editing a ghost's
  timing from a branch edits it everywhere.
- **Staffing is per-plan.** `Plan::block_rows` (lane per block) and
  `Plan::row_names` (lane names per drill scope) are the one dimension a
  branch owns independently. A fork snapshots main's rows; after that, main
  never writes a branch's rows again.
- **Dependencies are plan-local** (`Dependency::plan_id`). A branch dep never
  moves a main block — violation flagging only.

## Membership lifecycle

| Operation | Effect on membership |
|---|---|
| **Fork** (`fork_main(F)`) | Branch gets main's root blocks with `start_day >= F` (trunk stays behind, **Events-row blocks are never inherited**), plus a snapshot of main's `row_names` and each inherited block's lane. |
| **Create in main** (`link_main_block_to_branches`) | A *newly created* main block is appended as a ghost to every branch with `fork <= start_day`, seeded with main's lane. **Call only right after creation** — see the ambiguity rule below. Events never propagate. |
| **Remove from a branch** (`remove_block_from_plan`) | A ghost is just delisted from that branch (block lives on in main and siblings). An owned block is deleted outright if rooted nowhere else. |
| **Accept as main** (`accept_plan_as_main(B)`) | Main's future is rewritten: new main = trunk (`< F`) **+ main's events** + B's roster. B's staffing and deps are promoted (B wins); ghosts B removed are dropped from main (deleted if rooted nowhere); B is consumed. |
| **Rebase siblings** (`rebase_siblings_onto_main`) | After an accept, each sibling re-derives membership: keeps its owned blocks, keeps ghosts it already had, gains newly promoted blocks (`start_day >= its fork`), and honours its own past removals. Sibling deps with lost endpoints are pruned. |
| **Event prune** (`prune_event_ghosts_from_branches`) | Any ghost whose block sits on main's Events row leaves every branch (startup + drag-release sweep). Deps touching events survive — they re-anchor to the main event (cross-space edges). |

## The ambiguity rule (the load-bearing subtlety)

Membership is tracked positively (a branch's `root_blocks`) but **removals are
not recorded**. So "block absent from branch" is ambiguous: *removed by the
user* vs *never inherited*. Every operation must disambiguate from context:

- `link_main_block_to_branches` is only unambiguous at creation time (the id
  is brand new, so absence can only mean "not yet added"). Calling it on an
  existing block would resurrect deliberately removed ghosts.
- `rebase_siblings_onto_main` disambiguates with `old_main_roots`: a block in
  old main that the sibling lacks = deliberate removal (honour it); a block
  new to main = promoted from the accepted branch (add it).
- Events sidestep the rule entirely: they are *never* in a branch, so their
  absence is never a removal (this is why `accept` carries main's events
  through even though the branch doesn't list them).

**If #314 (splitting) mints new blocks or rewrites `start_day` across the
fork boundary, it must decide for each affected branch whether the result is
"inherited" or "removed" — the bookkeeping above will not do it for free.**

## Invariants (hold after every operation)

1. Every id in any plan's `root_blocks`, `block_rows`, or a dep endpoint
   exists in `work_blocks` (deps may be pruned to keep this).
2. A block rooted in **no** plan is deleted, along with its deps; children's
   `parent` links are cleared.
3. Main never gains an Events ghost dependency on a branch, and no branch
   ever roots an Events-row block.
4. `block_rows` never references a block outside that plan's roster.
5. Only main's own blocks propagate; branch-owned blocks never leak to
   siblings except by being promoted through an accept.

Tests enforcing these live in `model.rs` (`fork_*`, `propagation_*`,
`accept_*`, `rebase_*`, `prune_*`).
