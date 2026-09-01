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
uses UTC/en-US formatting, and disables animation/transition timing. Overview and
settings retain the existing light/dark smoke baselines; the route-family workflow
and analysis coverage captures the representative dark desktop/mobile states. All
screenshots use `expect(page).toHaveScreenshot(...)` through the shared helpers, and
`snapshotPathTemplate` keeps project-isolated baselines separate.

Run discovery with:

```sh
pnpm --dir apps/web exec playwright test --config=playwright.visual.config.ts --list
```

The axe test is backed by the typed `@axe-core/playwright` dependency and fails
on any serious or critical violation. The package metadata is already wired:

```sh
pnpm --dir apps/web test:visual
```

```sh
pnpm --dir apps/web test:visual:update
```

`test:visual:update` is the explicit companion for refreshing four project-scoped
goldens after an intentional UI change. Do not use it in CI; CI runs the regular
`test:visual` comparison.
