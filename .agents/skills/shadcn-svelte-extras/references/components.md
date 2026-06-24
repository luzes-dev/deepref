# shadcn-svelte-extras — Components Reference

Full prop tables, composition trees, and per-component notes for all 33 components in shadcn-svelte-extras.

## Table of Contents

- [Button](#button)
- [Chat](#chat)
- [Code](#code)
- [Confirm Delete Dialog](#confirm-delete-dialog)
- [Copy Button](#copy-button)
- [Emoji Picker](#emoji-picker)
- [Field Set](#field-set)
- [File Drop Zone](#file-drop-zone)
- [GitHub Button](#github-button)
- [Image Cropper](#image-cropper)
- [IPv4Address Input](#ipv4address-input)
- [Language Switcher](#language-switcher)
- [Light Switch](#light-switch)
- [Link](#link)
- [Meter](#meter)
- [Modal](#modal)
- [NLPDateInput](#nlpdateinput)
- [Number Field](#number-field)
- [Password](#password)
- [Phone Input](#phone-input)
- [PMCommand](#pmcommand)
- [Rename](#rename)
- [Snippet](#snippet)
- [Split Button](#split-button)
- [Star Rating](#star-rating)
- [Stepper](#stepper)
- [Tags Input](#tags-input)
- [Terminal](#terminal)
- [Theme Selector](#theme-selector)
- [Table of Contents](#table-of-contents-toc)
- [Tree View](#tree-view)
- [Underline Tabs](#underline-tabs)
- [Window](#window)

---

## Button

An extended button component. The same old button from shadcn-svelte with a few extra touches: a `loading` state and an `onClickPromise` that auto-tracks a promise's pending state.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/button`

**Composition:** Single component (`<Button.Root>` or `<Button>`).

**Behavior notes:**
- `loading={true}` disables the button and shows a spinner. Combine with `onClickPromise` for automatic loading state on async handlers.
- `onClickPromise` accepts a function returning a `Promise`. The button shows loading until the promise resolves or rejects. Rejecting leaves the button in a recoverable state — wrap with try/catch if you need to clear loading on failure.

### API — Button.Root

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLElement` | `null` |
| `children?` | `Snippet` | - |
| `variant?` | enum (`default` \| `outline` \| `ghost` \| `link` \| `secondary` \| `destructive`) | `default` |
| `size?` | enum (`default` \| `sm` \| `lg` \| `icon`) | `default` |
| `loading?` | `boolean` | - |
| `onClickPromise?` | `() => Promise<unknown>` | - |

---

## Chat

Components for creating live chats. Acknowledges inspiration from [`jakobhoeg/shadcn-chat`](https://github.com/jakobhoeg/shadcn-chat).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/chat`

**Composition:**
```
Chat.List
└── Chat.Bubble
    ├── Chat.BubbleAvatar
    │   ├── Chat.BubbleAvatarImage
    │   └── Chat.BubbleAvatarFallback
    └── Chat.BubbleMessage
```

### API

**Chat.List** — root container for chat messages.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |

**Chat.Bubble** — a chat bubble.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |
| `variant` | enum | - |

**Chat.BubbleMessage** — the message content within a chat bubble.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |
| `typing?` | `boolean` | `false` |

**Chat.BubbleAvatar** — avatar shown next to a chat bubble. Inherits all `Avatar.Root` props from bits-ui. See [bits-ui Avatar.Root](https://bits-ui.com/docs/components/avatar#root).

**Chat.BubbleAvatarImage** — re-export of `Avatar.Image`. See [bits-ui Avatar.Image](https://bits-ui.com/docs/components/avatar#image).

**Chat.BubbleAvatarFallback** — re-export of `Avatar.Fallback`. See [bits-ui Avatar.Fallback](https://bits-ui.com/docs/components/avatar#fallback).

---

## Code

A Shiki-powered code block component with syntax highlighting, copy button, optional line numbers, line highlighting, and overflow handling.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/code`

**Composition:**
```
Code.Overflow (optional)
└── Code.Root
    └── Code.CopyButton
```

**Configuring languages:** The Shiki highlighter and supported languages are configured from `shiki.ts` (created when the component is added). Edit this file to register additional languages or themes.

**Variants:**
- `default` — standard code block
- `outline` — bordered block
- `inline` — inline code

**Features:**
- `hideLines` — hide line numbers (they're shown by default)
- `highlight` — pass a single line number or `Array<[start, end]>` to highlight specific lines
- `Code.Overflow` — wraps `Code.Root` with a collapsed/expanded toggle; `bind:collapsed` controls state
- `Code.CopyButton` — standalone copy button (useful in custom layouts)

### API

**Code.Root** — the root code block component for syntax highlighting.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |
| `variant?` | enum (`default` \| `outline` \| `inline`) | `default` |
| `lang?` | enum (any Shiki-supported language) | `typescript` |
| `code` | `string` | - |
| `class?` | `string` | - |
| `hideLines?` | `boolean` | `false` |
| `highlight?` | `number \| [number, number][]` | `[]` |

**Code.CopyButton** — a button to copy the code block content.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLButtonElement` | `null` |
| `variant?` | enum | `ghost` |
| `size?` | enum | `icon` |
| `icon?` | `Snippet` | - |
| `animationDuration?` | `number` | - |
| `onCopy?` | `function` | - |
| `children?` | `Snippet` | - |

**Code.Overflow** — handles overflow of the code block (collapsed/expanded).

| Prop | Type | Default |
| --- | --- | --- |
| `collapsed?` `$bindable` | `boolean` | `true` |
| `children?` | `Snippet` | - |

---

## Confirm Delete Dialog

A dialog for confirming delete actions. Supports optional text-match confirmation (user must type a specific phrase to enable the delete button) and optional Shift+Click skip.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/confirm-delete-dialog`

**Composition:** Built on shadcn-svelte's `Dialog` and `Button` — use `Dialog.Root`/`Content`/etc. as needed and place the confirm UI inside.

**Variants:**
- **Default** — confirm dialog with Cancel / Delete buttons
- **With Text Confirmation** — force the user to type a specific phrase (e.g., the project name) before the Delete button enables
- **Skip Confirmation** — set `skipConfirmation` option to `true` and the user can Shift+Click the delete button to bypass the dialog entirely. Useful for power users.

The component does not publish its own prop table — it's a higher-level composition built on shadcn-svelte Dialog. Refer to the [shadcn-svelte Dialog docs](https://shadcn-svelte.com/docs/components/dialog) for the underlying dialog props.

---

## Copy Button

A button that copies text to the clipboard and shows feedback (icon animation, configurable duration).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/copy-button`

**Composition:** Single component. Provide `text` and an optional `icon` snippet (defaults to a checkmark/clipboard swap animation). Use `children` to show text alongside the icon.

**Custom icon:** Pass an `icon` snippet. The component animates between the icon and a success state for `animationDuration` ms.

**With text:** Set `children` to render a text label next to the icon.

### API — CopyButton

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLButtonElement` | `null` |
| `children?` | `Snippet` | - |
| `text` | `string` | - |
| `icon?` | `Snippet` | - |
| `animationDuration?` | `number` | - |
| `variant?` | enum (Button variants) | `ghost` |
| `size?` | enum (Button sizes) | `icon` |
| `onCopy?` | `function` | - |

---

## Emoji Picker

A composable emoji picker. Style inspired by [Frimousse](https://frimousse.liveblocks.app/).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/emoji-picker`

> **Note:** This component is large — the source includes a full emoji dataset. Use it where needed and avoid pulling it into route-level bundles where it's not used.

**Composition:**
```
EmojiPicker.Root
└── EmojiPicker.Viewport
    ├── EmojiPicker.Search
    ├── EmojiPicker.List
    └── EmojiPicker.Footer
        └── EmojiPicker.SkinToneSelector
```

**Features:**
- `bind:skin` — current skin tone (0 = none, 1–5 = light to dark)
- `onSelect(emoji)` — called when a user picks an emoji
- `showRecents` — show a recents category at the top
- `recentsKey` — localStorage key for recents (use to namespace per-user)
- `maxRecents` — cap on recent items
- `Popover` — wrap with shadcn-svelte `Popover` for a trigger-based picker

### API

**EmojiPicker.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `skin?` `$bindable` | enum (0–5) | `0` |
| `onSelect?` | `function` | - |
| `onSkinChange?` | `function` | - |
| `showRecents?` | `boolean` | - |
| `recentsKey?` | `string` | - |
| `maxRecents?` | `number` | - |
| `children?` | `Snippet` | - |

**EmojiPicker.List** — displays the list of emojis.

| Prop | Type | Default |
| --- | --- | --- |
| `emptyMessage?` | `string` | `No results.` |

**EmojiPicker.Viewport** — the viewport container for the emoji picker. No props.

**EmojiPicker.Search** — the search input for filtering emojis. Forwards to [bits-ui Command.Input](https://bits-ui.com/docs/components/command#input).

**EmojiPicker.Footer** — the footer area of the emoji picker.

| Prop | Type | Default |
| --- | --- | --- |
| `children?` | `Snippet` | - |

**EmojiPicker.SkinToneSelector** — button for selecting skin tone.

| Prop | Type | Default |
| --- | --- | --- |
| `previewEmoji?` | `string` | 👋 |

---

## Field Set

A field set component — groups a title, content area, and footer for settings-style forms.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/field-set`

**Composition:**
```
FieldSet.Root
├── FieldSet.Title
├── FieldSet.Content
└── FieldSet.Footer
```

**Variants:** `default` and `destructive` (use `destructive` for dangerous actions like "delete account").

### API

**FieldSet.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |
| `variant?` | enum (`default` \| `destructive`) | `default` |

**FieldSet.Title** — supports `level` prop (1–6) to pick the heading tag (`h1`–`h6`).

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLHeadingElement` | `null` |
| `children?` | `Snippet` | - |
| `level?` | enum (1–6) | `3` |

**FieldSet.Content**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |

**FieldSet.Footer**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |

---

## File Drop Zone

A file drop zone component — drag-and-drop, paste, or click-to-select file uploads with validation.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/file-drop-zone`

**Composition:**
```
FileDropZone.Root
├── FileDropZone.Trigger
└── FileDropZone.Textarea
```

**Behavior:**
- `onUpload(files: File[])` — called when files are selected. You handle the actual upload.
- `maxFiles`, `maxFileSize`, `accept` — validation props. Files that fail validation trigger `onFileRejected(file, reason)`.
- `fileCount` — current count (used to enforce `maxFiles` when adding incrementally).
- `FileDropZone.Textarea` — a textarea that accepts file uploads via drag-drop, paste, or click. Useful for chat-style "type your message + drop files" UIs. Integrates with Superforms — see [superforms.rocks — file uploads](https://superforms.rocks/concepts/files#file-uploads).

### API

**FileDropZone.Root** — renders a hidden file input element.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLInputElement` | `null` |
| `id?` | `string` | - |
| `children?` | `Snippet` | - |
| `onUpload` | `(files: File[]) => void` | - |
| `maxFiles?` | `number` | - |
| `fileCount?` | `number` | - |
| `maxFileSize?` | `number` | - |
| `onFileRejected?` | `function` | - |
| `accept?` | `string` | - |

**FileDropZone.Trigger** — renders as a label that activates the file input. Provides a default UI if no children are provided.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLLabelElement` | `null` |
| `children?` | `Snippet` | - |

**FileDropZone.Textarea** — textarea with file upload support.

| Prop | Type | Default |
| --- | --- | --- |
| `child?` | `Snippet` | - |
| `onpaste?` | `function` | - |
| `ondragover?` | `function` | - |
| `ondrop?` | `function` | - |

---

## GitHub Button

A button that displays the number of stars and links to a GitHub repository, with a tweened star count animation.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/github-button`

**Composition:** Single component.

**Icon only:** Omit `stars` to render just the GitHub logo (no star count).

### API — GithubButton

| Prop | Type | Default |
| --- | --- | --- |
| `repo` | `{ owner: string; name: string }` | - |
| `stars?` | `number` | - |
| `starsTweenedDuration?` | `number` | - |
| `variant?` | enum (Button variants) | `outline` |
| `size?` | enum (Button sizes) | `default` |
| `class?` | `string` | - |
| `ref?` `$bindable` | `HTMLAnchorElement` | `null` |
| `disabled?` | `boolean` | - |

---

## Image Cropper

A component for uploading and resizing images using [`svelte-easy-crop`](https://github.com/ValentinH/svelte-easy-crop). Inspired by [`sujjeee/shadcn-image-cropper`](https://github.com/sujjeee/shadcn-image-cropper).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/image-cropper`

**Composition:**
```
ImageCropper.Root
├── ImageCropper.UploadTrigger
│   └── ImageCropper.Preview
└── ImageCropper.Dialog
    ├── ImageCropper.Cropper
    └── ImageCropper.Controls
        ├── ImageCropper.Crop
        └── ImageCropper.Cancel
```

**Notes:**
- `bind:src` — bindable state for the cropped image (data URL or remote URL).
- `onCropped(dataUrl)` — called when the user confirms the crop.
- `onUnsupportedFile(file)` — called when the user drops a non-image file.
- **No Default Image** — omit `src` to start with an "Upload image" prompt instead of a preview.
- **Custom Trigger** / **Custom Preview** — replace the default trigger/preview with snippets.

### API

**ImageCropper.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `src?` `$bindable` | `string` | - |
| `onCropped?` | `function` | - |
| `onUnsupportedFile?` | `function` | - |
| `children?` | `Snippet` | - |

**ImageCropper.Dialog** — wraps the shadcn-svelte Dialog. Forwards all props to [bits-ui Dialog.Content](https://bits-ui.com/docs/components/dialog#content).

**ImageCropper.Cropper** — the cropper area. Forwards to [`svelte-easy-crop`](https://github.com/ValentinH/svelte-easy-crop).

**ImageCropper.Controls** — container for cropper controls.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `children?` | `Snippet` | - |

**ImageCropper.Preview** — displays a preview of the cropped image.

| Prop | Type | Default |
| --- | --- | --- |
| `child?` | `Snippet` | - |

**ImageCropper.UploadTrigger** — the trigger for uploading an image.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLLabelElement` | `null` |
| `children?` | `Snippet` | - |

---

## IPv4Address Input

An IPv4 address input with all the behavior you'd expect — segmented octets, automatic caret movement, paste handling, and built-in validation.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/ipv4address-input`

**Composition:** Single component.

**Props of note:**
- `bind:value` — the full IPv4 string (e.g., `"192.168.1.1"`)
- `bind:valid` — boolean that flips false when the address is malformed or incomplete
- `separator` — character between octets (default `.`)
- `placeholder` — placeholder for empty octets

### API — Ipv4addressInput

| Prop | Type | Default |
| --- | --- | --- |
| `separator?` | enum | `.` |
| `placeholder?` | `string` | - |
| `value?` `$bindable` | `string` | `null` |
| `class?` | `string` | - |
| `name?` | `string` | - |
| `valid?` `$bindable` | `boolean` | `false` |

---

## Language Switcher

A component for switching the site locale — a dropdown of languages.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/language-switcher`

**Composition:** Single component.

**Integration:**
- **Paraglide** — see [ParaglideJS docs](https://inlang.com/m/gerre34r/library-inlang-paraglideJs/sveltekit). The LanguageSwitcher docs page includes an example integration.
- `bind:value` — the currently selected language code.
- `onChange(code)` — fires when the user picks a language.
- `languages` — array of `{ code: string; label: string }` (the `Language[]` type).
- `align` — `"start"` or `"end"` (dropdown alignment).
- `variant` — button variant.

### API — LanguageSwitcher

| Prop | Type | Default |
| --- | --- | --- |
| `languages` | `Language[]` | - |
| `value?` `$bindable` | `string` | `''` |
| `align?` | enum (`start` \| `end`) | `end` |
| `variant?` | enum (Button variants) | `outline` |
| `onChange?` | `function` | - |
| `class?` | `string` | - |

---

## Light Switch

Click and change the theme — a sun/moon toggle button.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/light-switch`

**Composition:** Single component.

> **Required:** Include `<ModeWatcher />` from [`mode-watcher`](https://github.com/svecosystem/mode-watcher) in your root `+layout.svelte` so theme changes apply app-wide.

### API — LightSwitch

| Prop | Type | Default |
| --- | --- | --- |
| `variant?` | enum (Button variants) | `outline` |
| `size?` | enum (Button sizes) | `default` |

---

## Link

A simple styled anchor element. Crafted by [huntabyte](https://github.com/huntabyte), enhanced by [ieedan](https://github.com/ieedan).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/link`

**Composition:** Single component. Forwards all standard `<a>` attributes — see [MDN `<a>` element](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/a) for the full attribute reference.

### API — Link

No extra props beyond the standard `<a>` attributes. Use `href`, `target`, `rel`, etc. as usual.

---

## Meter

A styled meter element. Forwards to [bits-ui Meter](https://www.bits-ui.com/docs/components/meter#root).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/meter`

**Composition:** Single component.

Useful for showing progress toward a maximum (e.g., API tokens used vs. quota) — distinct from a progress bar (which represents task completion percentage).

### API — Meter

Forwards to [bits-ui Meter.Root](https://www.bits-ui.com/docs/components/meter#root). See bits-ui docs for the full prop surface.

---

## Modal

A responsive dialog component — Dialog on desktop, Drawer on mobile. `Modal.NestedRoot` is a drop-in replacement for `Root` when stacking modals (nested dialog on desktop, nested drawer on mobile).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/modal`

**Composition:**
```
Modal.Root / Modal.NestedRoot
├── Modal.Trigger
└── Modal.Content
    ├── Modal.Header
    │   ├── Modal.Title
    │   └── Modal.Description
    └── Modal.Footer
```

> If you want a plain Dialog (not responsive), use shadcn-svelte's `Dialog`, not `Modal`.

### API

All sub-components forward to their [bits-ui Dialog](https://www.bits-ui.com/docs/components/dialog#api-reference) counterparts.

- **Modal.Root** / **Modal.NestedRoot** → bits-ui Dialog.Root
- **Modal.Trigger** → bits-ui Dialog.Trigger
- **Modal.Content** → bits-ui Dialog.Content
- **Modal.Title** → bits-ui Dialog.Title
- **Modal.Description** → bits-ui Dialog.Description
- **Modal.Header** / **Modal.Footer** — styling wrappers, no special props.

---

## NLPDateInput

A natural-language date input with suggestions. Allows users to enter dates like "tomorrow at 5pm" or "in 2 hours" and get parsed suggestions.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/nlp-date-input`

**Composition:** Single component.

**Behavior:**
- `min` / `max` — clamp the suggestion range (so users don't schedule 30 years out).
- `onChoice(date)` — fired when the user picks a suggestion.
- `placeholder` — defaults to `E.g. "tomorrow at 5pm" or "in 2 hours"`.

### API — NlpDateInput

| Prop | Type | Default |
| --- | --- | --- |
| `min?` | `Date` | - |
| `max?` | `Date` | - |
| `placeholder?` | `string` | `E.g. "tomorrow at 5pm" or "in 2 hours"` |
| `onChoice?` | `function` | - |

---

## Number Field

A component for incrementing and decrementing a numeric value with `+`/`-` buttons. Supports ramp-on-hold (the longer you hold, the faster it increments).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/number-field`

**Composition:**
```
NumberField.Root
└── NumberField.Group
    ├── NumberField.Decrement
    ├── NumberField.Input
    └── NumberField.Increment
```

**Behavior:**
- `bind:value` — bindable numeric value.
- `rampBy` — how much to increment per ramp tick.
- `min` / `max` — clamp the value.
- `rampSettings` — fine-tune the ramp: `{ startDelay, rampUpTime, minFrequency, maxFrequency }` (all in ms). Defaults: `{ startDelay: 400, rampUpTime: 0, minFrequency: 35, maxFrequency: 35 }`.

The `useRamp` hook powers the ramp behavior — you can use it independently for custom hold-to-repeat UIs.

### API

**NumberField.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `value?` `$bindable` | `number` | - |
| `rampBy?` | `number` | - |
| `min?` | `number` | - |
| `max?` | `number` | - |
| `rampSettings?` | `object` | `{ startDelay: 400, rampUpTime: 0, minFrequency: 35, maxFrequency: 35 }` |
| `children?` | `Snippet` | - |

**NumberField.Group**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `class?` | `string` | - |
| `children?` | `Snippet` | - |

**NumberField.Input**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLInputElement` | `null` |
| `class?` | `string` | - |

**NumberField.Increment** / **NumberField.Decrement** — buttons with ramp behavior when held.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLElement` | `null` |
| `variant?` | enum | `ghost` |
| `size?` | enum | `icon` |
| `class?` | `string` | - |
| `children?` | `Snippet` | - |
| `disabled?` | `boolean` | `false` |
| `onpointerdown?` | `function` | - |
| `onpointerup?` | `function` | - |
| `onclick?` | `function` | - |

---

## Password

Components for handling passwords and other secrets — visibility toggle, copy button, and `zxcvbn-ts`-powered strength meter.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/password`

**Composition:**
```
Password.Root
├── Password.Input
│   ├── Password.Copy
│   └── Password.ToggleVisibility
└── Password.Strength
```

**Behavior:**
- `bind:hidden` — controls password visibility (`true` = masked, `false` = plain text).
- `bind:value` — the password string.
- `minScore` — `zxcvbn-ts` score threshold (0–4). When the password scores below `minScore`, the input is marked invalid and form submission is blocked. Default: `3`.
- `Password.Strength` — `bind:strength` exposes the full `ZxcvbnResult` (score, warning, suggestions, crack times, etc.).

### API

**Password.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLDivElement` | `null` |
| `hidden?` `$bindable` | `boolean` | `true` |
| `minScore?` | enum (0–4) | `3` |
| `children?` | `Snippet` | - |

**Password.Input**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLInputElement` | `null` |
| `value?` `$bindable` | `string` | - |
| `children?` | `Snippet` | - |

**Password.ToggleVisibility** — button to toggle password visibility. Forwards to [bits-ui Toggle](https://bits-ui.com/docs/components/toggle#root).

**Password.Copy** — copy-to-clipboard button. Forwards to the [Copy Button](#copy-button) component.

**Password.Strength** — meter that visually indicates password strength.

| Prop | Type | Default |
| --- | --- | --- |
| `strength?` `$bindable` | `ZxcvbnResult` | - |

---

## Phone Input

A phone number input component with country selector and built-in validation.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/phone-input`

**Composition:** Single component.

Acknowledges inspiration from [`omeralpi/shadcn-phone-input`](https://github.com/omeralpi/shadcn-phone-input).

**Behavior:**
- `bind:value` — the phone number string (E.164 or local format depending on `options`).
- `bind:country` — current country code (ISO 3166-1 alpha-2).
- `bind:valid` — boolean validity flag.
- `bind:detailedValue` — full parsed object (country code, national number, etc.).
- `defaultCountry` — initial country (does not bind).
- `order` — function to reorder the country list (e.g., put common countries first).
- `options` — `TelInputOptions` from the underlying `intl-tel-input`-style package.

### API — PhoneInput

| Prop | Type | Default |
| --- | --- | --- |
| `country?` `$bindable` | `CountryCode \| null` | `null` |
| `defaultCountry?` | `CountryCode \| null` | `null` |
| `name?` | `string` | - |
| `placeholder?` `$bindable` | `string` | - |
| `disabled?` `$bindable` | `boolean` | `false` |
| `readonly?` `$bindable` | `boolean` | `false` |
| `required?` | `boolean` | - |
| `class?` | `string` | - |
| `value?` `$bindable` | `string` | `""` |
| `valid?` `$bindable` | `boolean` | `false` |
| `detailedValue?` `$bindable` | `Partial<DetailedValue> \| null` | - |
| `options?` | `TelInputOptions` | `defaultOptions` |
| `order?` | `function` | - |

---

## PMCommand

A package-manager command component — tabs for npm/pnpm/yarn/bun with the active command displayed and a copy button. Uses [`package-manager-detector`](https://github.com/antfu-collective/package-manager-detector) to support every package manager and provide auto-complete for commands.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/pm-command`

**Composition:** Single component.

**Behavior:**
- `command` — the base command (e.g., `npx`).
- `args` — array of arguments (e.g., `["shadcn-svelte", "add", "button"]`).
- `agents` — array of supported package managers. Defaults to `['npm', 'pnpm', 'yarn', 'bun']`. Customize to limit (e.g., just `['pnpm', 'npm']`).
- `bind:agent` — currently selected agent.
- `variant` — `default` or other variants.
- **Persisted package manager** — use `runed`'s `PersistedState` API to remember the user's choice across sessions.
- **Overflow** — wraps with overflow handling for long commands.

### API — PmCommand

| Prop | Type | Default |
| --- | --- | --- |
| `variant?` | enum | `default` |
| `class?` | `string` | - |
| `agents?` | `Agent[]` | `['npm', 'pnpm', 'yarn', 'bun']` |
| `agent?` `$bindable` | `Agent` | `npm` |
| `command` | `Command` | - |
| `args` | `string[]` | - |

---

## Rename

A component for renaming stuff inline — toggle between a text view and an input. Supports content-editable mode (click text to edit), text-area mode, and external Edit/Save/Cancel controls.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/rename`

**Composition:**
```
Rename.Provider (optional, for external controls)
├── Rename.Root
├── Rename.Save
├── Rename.Cancel
└── Rename.Edit
```

`Rename.Root` is also exported as just `Rename`. For inline editing without a toolbar, use `Rename.Root` alone.

**Behavior:**
- `bind:value` — the current value.
- `bind:mode` — `"view"` or `"edit"`.
- `inputTag` — `"input"` (default) or `"textarea"`.
- `blurBehavior` — what happens when the input loses focus (e.g., save, cancel, or do nothing).
- `fallbackSelectionBehavior` — `"start"` or `"end"` (default `end`) — where the caret goes when entering edit mode.
- `onSave(value)` / `onCancel()` — callbacks.
- `validate(value)` — returns `true` to allow save, `false` to block. Default: `() => true`.
- **External Control** — `Rename.Provider` enables `Rename.Edit`, `Rename.Cancel`, and `Rename.Save` buttons to control a `Rename.Root` elsewhere in the tree via context. Useful for right-click "Rename" menus.

### API

**Rename.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `id?` | `string` | - |
| `this?` | enum | - |
| `inputTag?` | enum (`input` \| `textarea`) | `input` |
| `mode?` `$bindable` | enum (`view` \| `edit`) | `view` |
| `blurBehavior?` | enum | - |
| `fallbackSelectionBehavior?` | enum (`start` \| `end`) | `end` |
| `value` `$bindable` | `string` | - |
| `class?` | `string` | - |
| `inputClass?` | `string` | - |
| `textClass?` | `string` | - |
| `onSave?` | `function` | - |
| `onCancel?` | `function` | - |
| `validate?` | `(value: string) => boolean` | `() => true` |

**Rename.Provider** — context provider enabling external Edit/Cancel/Save buttons.

**Rename.Edit** / **Rename.Cancel** / **Rename.Save** — buttons that control the current Rename input via context.

| Prop | Type | Default |
| --- | --- | --- |
| `child?` | `Snippet` | - |

---

## Snippet

A snippet component — displays a code snippet (single-line or multi-line) with a copy button.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/snippet`

**Composition:** Single component.

**Behavior:**
- `text` — string or string array (multi-line). When given an array, each entry is its own line.
- `onCopy(text)` — fired after a successful copy.
- `variant` — visual variant.

### API — Snippet

| Prop | Type | Default |
| --- | --- | --- |
| `variant?` | enum | `default` |
| `text?` | `string \| string[]` | - |
| `class?` | `string` | - |
| `onCopy?` | `function` | - |

---

## Split Button

A split button — a primary action button plus a dropdown to pick which action runs. Use when you have one primary action and several secondary variants (e.g., "Merge" vs "Merge --squash" vs "Merge --rebase").

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/split-button`

**Composition:**
```
SplitButton.Root
├── SplitButton.Action   (the currently-selected action's content)
└── SplitButton.Select
    ├── SplitButton.SelectTrigger
    └── SplitButton.SelectContent
        └── SplitButton.SelectAction (one per action)
```

**Behavior:**
- `bind:value` — the currently selected action's value (matches the `value` prop of one of the `Action` / `SelectAction` children).
- `onActionSelect(value)` — fired when the user picks an action from the dropdown.
- `onClick` / `onClickPromise` — fired when the primary button is clicked. Pair with `onClickPromise` to show a loading state.
- `orientation` — `"horizontal"` (default) or `"vertical"`.

### API

**SplitButton.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLElement` | `null` |
| `children?` | `Snippet` | - |
| `orientation?` | enum (`horizontal` \| `vertical`) | `horizontal` |
| `value?` `$bindable` | `string` | - |
| `disabled?` | `boolean` | `false` |
| `onclick?` | `function` | - |
| `onClickPromise?` | `function` | - |
| `onActionSelect?` | `function` | - |

**SplitButton.Action** — only the action whose `value` matches the root's `value` is rendered.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLElement` | `null` |
| `value` | `string` | - |
| `children?` | `Snippet` | - |
| `onclick?` | `function` | - |

**SplitButton.Select** — wraps the dropdown primitive and binds its value to the root state.

| Prop | Type | Default |
| --- | --- | --- |
| `open?` `$bindable` | `boolean` | `false` |
| `children?` | `Snippet` | - |

**SplitButton.SelectTrigger** — button that opens the dropdown.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLElement` | `null` |
| `children?` | `Snippet` | - |

**SplitButton.SelectContent** — floating container for dropdown actions.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLElement` | `null` |
| `children?` | `Snippet` | - |

**SplitButton.SelectAction** — an option in the dropdown. Selecting it updates the active action.

| Prop | Type | Default |
| --- | --- | --- |
| `value` | `string` | - |
| `children?` | `Snippet` | - |

---

## Star Rating

A simple star rating component. Built on top of [bits-ui](https://bits-ui.com/)'s `<RatingGroup/>` so it has all the behaviors you'd expect — keyboard accessible, half-ratings, RTL support, disabled, readonly.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/star-rating`

**Composition:**
```
StarRating.Root
└── StarRating.Star
```

**Features:**
- **Custom count** — pass any number of stars (default is 5).
- **Half ratings** — supports `.5` values; works correctly in RTL layouts.
- **Disabled / readonly** — forwarded to bits-ui RatingGroup.
- **Custom color / size** — pass through `class` on `Root` or wrap each `Star` with custom styling.

### API

**StarRating.Root** — forwards all props and behavior to [bits-ui RatingGroup.Root](https://bits-ui.com/docs/components/rating-group#root). See bits-ui docs for the full prop surface (includes `value`, `max`, `allowHalf`, `disabled`, `readonly`, `onValueChange`, etc.).

**StarRating.Star** — a single star. Used internally to render each star with its state.

| Prop | Type | Default |
| --- | --- | --- |
| `index?` | `number` | - |
| `state?` | enum (`active` \| `inactive` \| `placeholder`) | - |
| `class?` | `string` | - |

---

## Stepper

A keyboard-accessible stepper component — multi-step indicators for wizards, multi-step forms, onboarding flows.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/stepper`

**Composition:**
```
Stepper.Root
├── Stepper.Nav
│   └── Stepper.Item
│       ├── Stepper.Trigger
│       │   ├── Stepper.Indicator
│       │   ├── Stepper.Title
│       │   └── Stepper.Description
│       └── Stepper.Separator
├── Stepper.Previous
└── Stepper.Next
```

**Behavior:**
- `bind:step` — current step index (0-based).
- **Icons** — put an icon inside `Stepper.Indicator` to show a checkmark or custom icon instead of the step number.
- **Vertical** — set `orientation="vertical"` on `Stepper.Nav`.
- **Forms** — combine with SvelteKit form actions or `superforms` for multi-step forms. Each step renders its own form section; `Stepper.Next` advances.

### API

**Stepper.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `step?` `$bindable` | `number` | - |
| `children?` | `Snippet` | - |

**Stepper.Nav** — controls orientation of the stepper.

| Prop | Type | Default |
| --- | --- | --- |
| `orientation?` | enum (`horizontal` \| `vertical`) | `horizontal` |

**Stepper.Item** — a single step.

| Prop | Type | Default |
| --- | --- | --- |
| `id?` | `string` | - |

**Stepper.Trigger** — clickable trigger button for a step.

| Prop | Type | Default |
| --- | --- | --- |
| `ref?` `$bindable` | `HTMLButtonElement` | `null` |
| `onclick?` | `function` | - |
| `onkeydown?` | `function` | - |
| `children?` | `Snippet` | - |

**Stepper.Indicator** / **Stepper.Separator** / **Stepper.Title** / **Stepper.Description** — visual sub-components.

| Prop | Type | Default |
| --- | --- | --- |
| `children?` | `Snippet` | - |

**Stepper.Next** — navigates to the next step. Auto-disabled on last step.

| Prop | Type | Default |
| --- | --- | --- |
| `disabled?` | `boolean` | `false` |
| `variant?` | enum (Button variants) | `default` |
| `size?` | enum (Button sizes) | `default` |
| `child?` | `Snippet` | - |
| `children?` | `Snippet` | - |

**Stepper.Previous** — navigates to the previous step. Auto-disabled on first step.

| Prop | Type | Default |
| --- | --- | --- |
| `disabled?` | `boolean` | `false` |
| `variant?` | enum (Button variants) | `outline` |
| `size?` | enum (Button sizes) | `default` |
| `child?` | `Snippet` | - |
| `children?` | `Snippet` | - |

---

## Tags Input

A tags input component — chips with add/remove, custom validation, autocomplete suggestions, and a "restrict to suggestions" mode.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/tags-input`

**Composition:** Single component.

**Behavior:**
- `bind:value` — `string[]` of current tags.
- `validate(value)` — transform/validate function. Returns the cleaned value to add, or `false`/throws to reject. Default: `defaultValidate`. Use this for normalization (lowercase, trim, dedupe).
- `onValueChange(tags)` — fired when the tag list changes.
- `suggestions` — array of strings for autocomplete.
- `filterSuggestions(input, suggestions)` — custom filter function for the dropdown.
- `restrictToSuggestions` — when `true`, only values from `suggestions` are accepted.

### API — TagsInput

| Prop | Type | Default |
| --- | --- | --- |
| `value?` `$bindable` | `string[]` | `[]` |
| `validate?` | `function` | `defaultValidate` |
| `onValueChange?` | `function` | - |
| `suggestions?` | `string[]` | - |
| `filterSuggestions?` | `function` | - |
| `restrictToSuggestions?` | `boolean` | `false` |

---

## Terminal

A macOS-style terminal implementation — useful for showcasing a CLI in a landing page or docs. Inspired by [`magicuidesign/magicui`](https://magicui.design/docs/components/terminal).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/terminal`

**Composition:**
```
Terminal.Loop (optional, wraps to repeat the animation)
└── Terminal.Root
    ├── Terminal.TypingAnimation
    ├── Terminal.AnimatedSpan
    └── Terminal.Loading
```

**Behavior:**
- `Terminal.Root` — `delay` (ms) delays every animation by that amount; `speed` (higher = faster) changes animation speed of all children; `onComplete()` fires when all animations finish (once per run unless wrapped in `Loop`).
- `Terminal.TypingAnimation` — typewriter effect for its children (use for command input).
- `Terminal.AnimatedSpan` — fly-in effect for its children (use for output lines).
- `Terminal.Loading` — animated loading state with `loadingMessage` and `completeMessage` snippets, plus `duration`.
- `Terminal.Loop` — repeats its children. Use to make the demo continuously loop instead of running once.

### API

**Terminal.Root**

| Prop | Type | Default |
| --- | --- | --- |
| `children?` | `Snippet` | - |
| `class?` | `string` | - |
| `delay?` | `number` | - |
| `speed?` | `number` | - |
| `onComplete?` | `function` | - |

**Terminal.TypingAnimation** / **Terminal.AnimatedSpan**

| Prop | Type | Default |
| --- | --- | --- |
| `children?` | `Snippet` | - |
| `class?` | `string` | - |
| `delay?` | `number` | - |

**Terminal.Loading**

| Prop | Type | Default |
| --- | --- | --- |
| `loadingMessage` | `Snippet` | - |
| `completeMessage` | `Snippet` | - |
| `duration?` | `number` | - |
| `delay?` | `number` | - |
| `class?` | `string` | - |

**Terminal.Loop**

| Prop | Type | Default |
| --- | --- | --- |
| `children?` | `Snippet` | - |
| `delay?` | `number` | - |

---

## Theme Selector

A dropdown to select light / dark / system theme.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/theme-selector`

**Composition:** Single component.

> **Required:** Include `<ModeWatcher />` from [`mode-watcher`](https://github.com/svecosystem/mode-watcher) in your root `+layout.svelte` so the selected theme applies app-wide.

### API — ThemeSelector

| Prop | Type | Default |
| --- | --- | --- |
| `variant?` | enum (Button variants) | `outline` |
| `size?` | enum (Button sizes) | `default` |

---

## Table of Contents (Toc)

A component for displaying a table of contents. Pairs with the [`useToc`](hooks.md#usetoc) hook, which generates the `Heading[]` from page content.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/toc`

**Composition:** Single component.

**Behavior:**
- `toc` — `Heading[]` produced by `useToc`.
- `isChild` — rendering hint for nested TOCs (default `false`).
- `class` — extra class.

### API — Toc

| Prop | Type | Default |
| --- | --- | --- |
| `toc` | `Heading[]` | - |
| `class?` | `string` | - |
| `isChild?` | `boolean` | `false` |

---

## Tree View

A file-tree component with `Folder` and `File` nodes, expand/collapse, and custom icon snippets.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/tree-view`

**Composition:**
```
TreeView.Root
└── TreeView.Folder
    └── TreeView.File
```

**Behavior:**
- `Folder` — `name`, `bind:open` (expanded state, default `false`), `icon` snippet (custom folder icon), `children` (nested folders/files).
- `File` — `name`, `icon` snippet (custom file icon), `children`.
- **Custom icons** — if you have a project-wide set of file icons (e.g., per-extension), wrap `Folder`/`File` with your own component that injects the right `icon` snippet.

### API

**TreeView.Root** — root of the tree. No special props.

**TreeView.Folder** — a folder node.

| Prop | Type | Default |
| --- | --- | --- |
| `name` | `string` | - |
| `open?` `$bindable` | `boolean` | `false` |
| `class?` | `string` | - |
| `icon?` | `Snippet` | - |
| `children?` | `Snippet` | - |

**TreeView.File** — a file node. Renders as a button.

| Prop | Type | Default |
| --- | --- | --- |
| `name` | `string` | - |
| `icon?` | `Snippet` | - |
| `children?` | `Snippet` | - |

---

## Underline Tabs

Horizontal tabs with an animated underline.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/underline-tabs`

**Composition:**
```
UnderlineTabs.Root
├── UnderlineTabs.List
│   └── UnderlineTabs.Trigger
└── UnderlineTabs.Content
```

All sub-components forward to their [bits-ui Tabs](https://www.bits-ui.com/docs/components/tabs) counterparts.

- **UnderlineTabs.Root** → bits-ui Tabs.Root (see [docs](https://www.bits-ui.com/docs/components/tabs#root))
- **UnderlineTabs.List** → bits-ui Tabs.List
- **UnderlineTabs.Trigger** → bits-ui Tabs.Trigger
- **UnderlineTabs.Content** → bits-ui Tabs.Content

**Overflow:** The component supports overflow handling for when the tab list is wider than the container.

---

## Window

A macOS-style window chrome wrapper — useful for previews, landing-page demos, and styled content containers.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/window`

**Composition:** Single component.

### API — Window.Root

| Prop | Type | Default |
| --- | --- | --- |
| `contentClass?` | `string` | - |
| `children?` | `Snippet` | - |
