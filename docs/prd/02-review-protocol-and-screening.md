# Review protocol and screening

## Goal

Make eligibility decisions reproducible, fast to review, and safe under
concurrent edits.

## Requirements

- Protocols use a framework discriminator and structured fields.
- Criteria are ordered objects with kind, stage, dimension, label, and
  description.
- Publishing locks a protocol version. Later edits create an amendment.
- Every screening decision records the protocol version, actor, revision,
  notes, and resulting state.
- Title and abstract screening supports unscreened, exclude, maybe, and
  include. Maybe is not include.
- Full-text screening requires one standardized primary exclusion reason.
- Decision history is append-only. Undo creates a superseding event.
- Focus mode, table mode, keyboard actions, filters, pagination, and optimistic
  updates operate on the same server state.
- Stale writes return the authoritative state and do not overwrite history.

## Invariants

- Eligibility is project-specific and protocol-version-specific.
- A conflict never becomes a successful decision.
- A protocol edit cannot silently alter a past decision.

## Acceptance evidence

- Protocol implementation and tests:
  `apps/web/src/lib/features/protocol/` and
  `apps/web/src/routes/projects/[projectId]/protocol/page.svelte.e2e.ts`.
- Screening implementation and tests:
  `crates/postgres/tests/screening.rs`,
  `apps/web/src/lib/features/screening/`, and the title/abstract and full-text
  Playwright suites.
- Run:

```text
cargo test -p deepref-postgres --test screening --locked
pnpm --filter @deepref/web check
pnpm --filter @deepref/web test:e2e -- --grep 'conflict|protocol'
```
