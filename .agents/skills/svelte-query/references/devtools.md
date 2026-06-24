# Devtools reference

How to install, mount, and configure `@tanstack/svelte-query-devtools`. Read this when the user wants to inspect query cache state, debug stale/refetch behavior, or trigger mock errors.

## Table of contents
- Install
- Browser extensions (alternative)
- Mount
- Options
- Usage tips
- SSR considerations

## Install

Devtools is a separate package:

```bash
npm i @tanstack/svelte-query-devtools
# or: pnpm add @tanstack/svelte-query-devtools / yarn add / bun add
```

Then import:

```ts
import { SvelteQueryDevtools } from '@tanstack/svelte-query-devtools'
```

## Browser extensions (alternative)

For Chrome, Firefox, and Edge, third-party browser extensions provide the same UI without adding a package to your app:

- Chrome: https://chromewebstore.google.com/detail/tanstack-query-devtools/annajfchloimdhceglpgglpeepfghfai
- Firefox: https://addons.mozilla.org/en-US/firefox/addon/tanstack-query-devtools/
- Edge: https://microsoftedge.microsoft.com/addons/detail/tanstack-query-devtools/edmdpkgkacmjopodhfolmphdenmddobj

These detect any TanStack Query app on the page and give you the same devtools panel.

## Mount

Place `<SvelteQueryDevtools />` **inside** the `<QueryClientProvider>`. As high in the tree as possible — usually right next to the children slot:

```svelte
<script lang="ts">
  import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query'
  import { SvelteQueryDevtools } from '@tanstack/svelte-query-devtools'

  const queryClient = new QueryClient()
</script>

<QueryClientProvider client={queryClient}>
  <!-- The rest of your application -->
  <SvelteQueryDevtools />
</QueryClientProvider>
```

By default, devtools mounts as a **floating** element in the corner of the screen with a toggle button. The toggle state persists in `localStorage` across reloads.

In production builds, the devtools panel still renders but is collapsed by default — you usually want to conditionally include it only in dev:

```svelte
<script lang="ts">
  import { dev } from '$app/environment'
  import { SvelteQueryDevtools } from '@tanstack/svelte-query-devtools'
</script>

{#if dev}
  <SvelteQueryDevtools />
{/if}
```

Or with Vite's `import.meta.env.DEV`:

```svelte
{#if import.meta.env.DEV}
  <SvelteQueryDevtools />
{/if}
```

## Options

All optional. Pass as props.

### `initialIsOpen: boolean`

Default `false`. If `true`, the devtools panel starts expanded on mount.

```svelte
<SvelteQueryDevtools initialIsOpen={true} />
```

### `buttonPosition: 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right' | 'relative'`

Default `'bottom-right'`. Where the toggle button sits. Use `'relative'` if you want to place the button yourself — it renders wherever the component is mounted.

```svelte
<SvelteQueryDevtools buttonPosition="top-right" />
```

### `position: 'top' | 'bottom' | 'left' | 'right'`

Default `'bottom'`. Where the panel slides out from relative to the button.

```svelte
<SvelteQueryDevtools position="right" />
```

### `client?: QueryClient`

Default: the client from the nearest `QueryClientProvider` context. Pass an explicit client if you have multiple and want to inspect a specific one.

```svelte
<SvelteQueryDevtools client={myOtherQueryClient} />
```

### `errorTypes?: { name: string; initializer: (query: Query) => TError }[]`

Define mock error types that can be triggered from the devtools UI to test your error handling. When the user toggles an error on, `initializer` is called with the query and must return an `Error`.

```svelte
<script lang="ts">
  const errorTypes = [
    {
      name: 'NetworkError',
      initializer: () => new Error('Simulated network failure'),
    },
    {
      name: '401Unauthorized',
      initializer: () => new Error('Unauthorized'),
    },
  ]
</script>

<SvelteQueryDevtools {errorTypes} />
```

Once defined, the devtools UI exposes buttons to inject these errors into any active query — useful for testing your `onError` callbacks and error UI.

### `styleNonce?: string`

Pass a nonce to the `<style>` tag the devtools injects into `<head>`. Use when your Content Security Policy disallows inline styles without a nonce.

```svelte
<SvelteQueryDevtools styleNonce={cspNonce} />
```

### `shadowDOMTarget?: ShadowRoot`

Default: styles are applied to the document `<head>`. If your app lives in a shadow DOM, pass the `ShadowRoot` so devtools styles are scoped to it instead of leaking into the light DOM.

```svelte
<script lang="ts">
  let shadowRoot: ShadowRoot
  // …attach shadow root somehow…
</script>

<SvelteQueryDevtools shadowDOMTarget={shadowRoot} />
```

## Usage tips

- **Filter by query key.** The panel has a search box — type part of a query key to narrow down.
- **Inspect status, data, error, dataUpdatedAt.** Click any query to see its full state.
- **Manual refetch / invalidate.** Buttons on each query let you trigger refetch or invalidate without touching the app.
- **Trigger mock errors.** Configure `errorTypes` to inject errors and verify your UI.
- **See mutation history.** Mutations show up in a separate tab — useful for confirming `onMutate`/`onError`/`onSuccess` ran in the right order.
- **Watch cache GC.** When a query has no observers and `gcTime` elapses, it disappears from the panel.

## SSR considerations

`<SvelteQueryDevtools />` accesses browser-only APIs (DOM, localStorage) and should not be rendered on the server. Wrap it in a `browser` check:

```svelte
<script lang="ts">
  import { browser } from '$app/environment'
  import { SvelteQueryDevtools } from '@tanstack/svelte-query-devtools'
</script>

{#if browser}
  <SvelteQueryDevtools />
{/if}
```

Or import it dynamically only on the client:

```svelte
<script lang="ts">
  import { onMount } from 'svelte'
  let Devtools: any

  onMount(async () => {
    const mod = await import('@tanstack/svelte-query-devtools')
    Devtools = mod.SvelteQueryDevtools
  })
</script>

{#if Devtools}
  <Devtools />
{/if}
```

The dynamic-import approach also keeps the devtools package out of your SSR bundle, which is a small win.
