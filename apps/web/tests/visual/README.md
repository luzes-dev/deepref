# Deterministic visual harness

`playwright.config.ts` owns both the ordinary `e2e` project and the fixed
visual projects. The regular `test:e2e` script selects only `e2e`, so the route-
level scenarios run once; `playwright.visual.config.ts` is a focused entrypoint
that reuses the same visual project definitions:

- `visual-dark-desktop-1440x1100`
- `visual-light-desktop-1440x1100`
- `visual-dark-mobile-390x844`
- `visual-light-mobile-390x844`

The fixture intercepts every `**/api/**` request, serves stable project/article/
ingestion/projection/review data, freezes `Date.now()`, supplies deterministic UUIDs,
uses UTC/en-US formatting, and disables animation/transition timing. Unmodeled API
requests, including mutations, fail the fixture instead of receiving a permissive
success response.

Overview and settings retain light/dark screenshot baselines. Route-family workflow
and analysis coverage retains representative dark desktop/mobile screenshot baselines
while all four projects execute their behavioral, theme, overflow, and accessibility
assertions. This is intentionally not described as light-mode visual regression:
light workflow coverage verifies the explicit light theme contract plus interaction
and axe behavior until light workflow PNG goldens are explicitly generated and
reviewed. All committed screenshots use `expect(page).toHaveScreenshot(...)` through
the shared helpers, and `snapshotPathTemplate` keeps project-isolated baselines
separate.

Run discovery with:

```sh
pnpm --dir apps/web exec playwright test --config=playwright.visual.config.ts --list
```

The axe coverage is backed by the typed `@axe-core/playwright` dependency and fails
on any serious or critical violation. Workflow-family routes run this gate in every
visual project. The package metadata is already wired:

```sh
pnpm --dir apps/web test:visual
```

```sh
pnpm --dir apps/web test:visual:update
```

`test:visual:update` is the explicit companion for refreshing project-scoped goldens
after an intentional UI change. Do not use it in CI. The required full-stack browser
job runs `test:e2e` and then the regular `test:visual` comparison/accessibility suite.
