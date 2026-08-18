# DeepRef Web

SvelteKit frontend for DeepRef.

The app is an SPA behind the web gateway and uses the same-origin `/api` contract. It polls dependency status independently, keeps core project/ingestion/settings navigation usable during graph recovery, and renders typed graph-only degradation and metric freshness. Cloudflare Access is the hosted perimeter; the web app has no user or tenant model.

```bash
mise exec -- just bootstrap
mise exec -- just dev
```

Package-local commands:

```bash
pnpm --filter @deepref/web lint
pnpm --filter @deepref/web check
pnpm --filter @deepref/web test
pnpm --filter @deepref/web build
```

Run package-local commands through `mise exec --` so they use the repository's pinned Node and pnpm versions.

The API client under `src/lib/api/generated` is Orval-generated. Regenerate and check it from the repository root with `mise exec -- just codegen` and `mise exec -- just codegen-check`; do not hand-edit generated files.
