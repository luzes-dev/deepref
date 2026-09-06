# Web Linting and Typecheck Profiling Report

This document records the profiling findings, bottleneck analysis, and optimizations for web linting (`pnpm run lint`) and typechecking (`pnpm run check`) in `apps/web`.

## Baseline Performance

Profiling was conducted on an x86_64 Linux workstation:

| Command | Subcommand | Baseline Duration | CPU Time | Memory Peak |
| :--- | :--- | :--- | :--- | :--- |
| `pnpm run lint` | `prettier --check .` | 9.4s | 12.6s | ~150 MB |
| `pnpm run lint` | `eslint .` | 61.1s | 92.7s | ~2.7 GB |
| **`pnpm run lint` Total** | (Sequential) | **70.5s** | **105.3s** | **~2.7 GB** |
| **`pnpm run check` Total** | `svelte-check` | **12.5s** | **18.6s** | **~600 MB** |

## Bottleneck Analysis

### 1. ESLint Parser & Project Service Overhead
- Using `TIMING=1` showed rule execution alone took only 5.5s across all lint rules.
- The remaining ~55s was consumed entirely by AST parsing, module resolution, and TypeScript Language Service program generation through `projectService: true`.
- ESLint was evaluating every file in `apps/web/`, including 234 generated Orval API client TypeScript files (`src/lib/api/generated/**`) and internal `.svelte-kit` route manifests.

### 2. Missing Caching
- Neither `prettier` nor `eslint` was executed with caching flags (`--cache`).
- Every invocation re-parsed and re-checked every unmodified file from scratch, even when zero source files had changed.

### 3. Gitignore Scope and Root Pathing
- `apps/web/eslint.config.js` previously referenced `path.resolve('../../.gitignore')`.
- Because paths in `.gitignore` are relative to the repository root, local ignore patterns such as `/.svelte-kit/` were not resolved against the workspace directory `apps/web/`, causing ESLint's configuration loader to traverse generated framework directories.

## Optimizations Applied

1. **Persistent Cache Flags**:
   - Updated `apps/web/package.json` scripts:
     - `"lint": "prettier --check --cache . && eslint --cache ."`
     - `"format": "prettier --write --cache ."`
   - Added `.eslintcache` and `.prettiercache` to both root and `apps/web/.gitignore`.

2. **Scoped Ignore Rules in ESLint**:
   - Added explicit scoped ignore rules in `apps/web/eslint.config.js` for:
     - `.svelte-kit/**`
     - `build/**` and `dist/**`
     - `.stryker-tmp/**`
     - `node_modules/**`
     - `src/lib/api/generated/**` (generated API client verified by `check-api-codegen.sh` and typechecked by `svelte-check`)
   - Included both workspace-local `.gitignore` and root `.gitignore` via `includeIgnoreFile`.

## Post-Optimization Results

| Step | Cold Duration | Warm/Incremental Duration | Speedup |
| :--- | :--- | :--- | :--- |
| `prettier --check --cache .` | 9.6s | 3.4s | 2.8x |
| `eslint --cache .` | 58.4s | 1.5s | **39x** |
| **`pnpm run lint` Total** | ~68s | **~4.9s** | **14.3x** |

Correctness invariants remain unchanged:
- `pnpm run check` (`svelte-check`) continues to typecheck 100% of workspace modules including generated API clients with 0 errors and 0 warnings.
- Prettier continues checking all authored source files.
