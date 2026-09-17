# UI review

This pass focuses on a quiet, readable research workspace with task controls close to the evidence. It retains the existing Svelte, shadcn-svelte, IBM Plex Sans, and teal identity.

## References

- [Dribbble research dashboards](https://dribbble.com/tags/research-dashboard): inspected the Movade research workspace and Academic SaaS concepts for navigation, content grouping, and placement of search.
- [Awwwards minimal gallery](https://www.awwwards.com/websites/minimal/): explored restrained typography and spacing references. Product tasks remain the priority over decorative motion.

## Page decisions

| Page               | Change                                                                                                               |
| ------------------ | -------------------------------------------------------------------------------------------------------------------- |
| Overview           | Direct entry to screening, with shortcuts to imports, connections, and reporting. Metrics are secondary.             |
| Protocol           | Smaller heading and quieter context labels give the protocol form more space.                                        |
| Articles           | Removed duplicate metric tiles; search and the article table appear earlier. Inspector appears only after selection. |
| Imports            | Removed redundant counters; plain instructions explain DOI entry. Inspector appears only for a selected run.         |
| Duplicates         | Clear explanation of what to compare and how to start checking.                                                      |
| Title and abstract | Compact progress row, wider reading area, optional AI assistance.                                                    |
| Full text          | More compact header leaves more room for the source and decision controls.                                           |
| Studies            | Existing groups take priority; new-group form opens on request.                                                      |
| Appraisal          | Clearer instructions for selecting an article and assessment framework.                                              |
| Extraction         | Plain explanations of study selection, fields, proposals, and saved values.                                          |
| PRISMA             | Diagram and exports precede exhaustive counts, which remain available in a disclosure.                               |
| Graph              | Graph comes before optional overlay, legend, and update details.                                                     |
| Recommendations    | Wrapped article titles, fewer nested scroll areas, update details available on request.                              |
| Automations        | Clearer maintenance setup and run language.                                                                          |
| Assistant          | Describes useful actions and review responsibility instead of internal tool contracts.                               |
| Settings           | Added a return link to the workspace.                                                                                |

Shared changes include consistent navigation links and active states, quieter light and dark palettes, and reduced decorative borders and metric colors. Reduced-motion and keyboard focus support are retained.

## Local sample workspace

`scripts/seed-ui.sql` creates a fictional ambient documentation review with 12 articles, 22 citation links, a published PICO protocol, mixed screening states, two study groups, and extraction fields. The data is explicitly marked as sample evidence. It is idempotent and retains existing decisions.

The review instance uses a dedicated `deepref-human-touch-postgres` container, database `deepref_ui_review`, PostgreSQL port 5443, API port 8083, and web port 5183. This avoids other worktrees' development services.

```bash
# Restart the existing dedicated database:
docker start deepref-human-touch-postgres

# Seed after migrations:
docker exec -i deepref-human-touch-postgres psql -v ON_ERROR_STOP=1 -U postgres -d deepref_ui_review < scripts/seed-ui.sql

# API (run from repository root):
APP_ENV=local DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5443/deepref_ui_review API_BIND_ADDR=127.0.0.1:8083 DOCUMENT_STORAGE_BACKEND=local DOCUMENT_STORAGE_ROOT=/tmp/deepref-ui-documents target/debug/deepref-server serve

# Worker (another terminal, for background tasks):
APP_ENV=local DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5443/deepref_ui_review DOCUMENT_STORAGE_BACKEND=local DOCUMENT_STORAGE_ROOT=/tmp/deepref-ui-documents target/debug/deepref-server worker

# Web (another terminal):
API_PROXY_TARGET=http://127.0.0.1:8083 mise exec -- pnpm dev:web --host 127.0.0.1 --port 5183
```

Open `/projects/00000000-0000-4000-8000-000000000201/overview` on the web server.

## Verification

The full browser suite passes 65 tests, and the unit suite passes 122 tests. Desktop and mobile browser inspection covers the workflow pages, light and dark themes, and article selection. Automated accessibility checks report no serious or critical violations for the checked overview, mobile article list, and open mobile inspector. Phone inspectors use the full screen width, with keyboard-accessible scrolling.

## Scroll and section-layout follow-up

The desktop workspace now owns a bounded scrolling region for route content, including screening and appraisal pages that previously grew inside clipped panes. Long form pages flow naturally inside it. Mobile uses the same viewport.

Protocol uses Research question, Framework, and Eligibility criteria tabs; Extraction uses Fields, Review proposals, and Accepted values; Automations uses Setup, Run now, and Run history; Recommendations separates its three reading groups into tabs; Appraisal separates the assessment from optional AI suggestions. Form state remains in the parent screen while switching tabs. Major task containers are semantic sections rather than decorated cards. Shared surfaces and metrics are flat, and redundant header breadcrumbs and study summary tiles are removed.

Browser checks at a 600px desktop height and a 390px-wide mobile viewport verified that all overflowing regions on the 15 workflow routes could reach their bottom. The protocol browser test also exercises reaching Save draft at a 500px height.
