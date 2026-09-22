<script lang="ts">
	import { page } from '$app/state';
	import { onNavigate } from '$app/navigation';
	import * as Dialog from '@deepref/ui/dialog';
	import SettingsView from './SettingsView.svelte';
	import {
		closeSettingsOverlay,
		consumeSettingsTransition,
		expandSettings,
		getSettingsTrigger
	} from './navigation';

	const open = $derived(Boolean(page.state.settingsOverlay));

	function shouldAnimateSettingsNavigation(): boolean {
		return (
			typeof document !== 'undefined' &&
			typeof window !== 'undefined' &&
			typeof document.startViewTransition === 'function' &&
			typeof window.matchMedia === 'function' &&
			!window.matchMedia('(prefers-reduced-motion: reduce)').matches
		);
	}

	// Let SvelteKit finish the real route update inside the browser's native
	// view transition. This keeps the old modal painted until the full page is
	// ready, with no artificial timeout or blank intermediate state.
	onNavigate(({ complete }) => {
		if (consumeSettingsTransition() === null || !shouldAnimateSettingsNavigation()) {
			return;
		}

		// SvelteKit waits for this promise before updating the component tree.
		// Resolving from inside the update callback lets the browser capture the
		// modal first; awaiting `complete` then keeps the transition alive until
		// SvelteKit has painted the full-page destination.
		return new Promise<void>((resolve) => {
			try {
				const transition = document.startViewTransition(async () => {
					resolve();
					await complete;
				});

				// A failed or superseded navigation must not create unhandled
				// rejections. The browser releases its transition pseudo-elements
				// when each lifecycle promise settles.
				void transition.ready.catch(() => undefined);
				void transition.updateCallbackDone.catch(() => undefined);
				void transition.finished.catch(() => undefined);
			} catch {
				// Browsers can reject a second transition while one is active. The
				// navigation itself remains valid and continues without animation.
				resolve();
			}
		});
	});

	function handleOpenChange(nextOpen: boolean): void {
		if (!nextOpen) closeSettingsOverlay();
	}

	function restoreTriggerFocus(event: Event): void {
		if (page.state.settingsExpansion) {
			event.preventDefault();
			return;
		}

		const trigger = getSettingsTrigger();
		if (trigger && document.contains(trigger)) {
			event.preventDefault();
			trigger.focus();
			return;
		}

		const mobileTrigger = document.querySelector<HTMLElement>(
			'[data-testid="mobile-navigation-trigger"]'
		);
		if (mobileTrigger) {
			event.preventDefault();
			mobileTrigger.focus();
		}
	}
</script>

{#if open}
	<Dialog.Root {open} onOpenChange={handleOpenChange}>
		<Dialog.Content
			showCloseButton={false}
			onCloseAutoFocus={restoreTriggerFocus}
			class="h-[min(600px,calc(100dvh-2rem))] w-[min(52.625rem,calc(100vw-2rem))] max-w-none gap-0 overflow-hidden p-0 sm:max-w-none"
			data-testid="settings-dialog"
		>
			<Dialog.Header class="sr-only">
				<Dialog.Title>Settings</Dialog.Title>
				<Dialog.Description>
					Manage application settings without leaving the current workspace.
				</Dialog.Description>
			</Dialog.Header>
			<div
				class="h-full w-full"
				style="view-transition-name: settings-panel;"
				data-testid="settings-dialog-panel"
			>
				<SettingsView
					presentation="modal"
					onclose={closeSettingsOverlay}
					onexpand={expandSettings}
				/>
			</div>
		</Dialog.Content>
	</Dialog.Root>
{/if}

<style>
	:global(::view-transition-group(settings-panel)) {
		animation-duration: 220ms;
		animation-timing-function: cubic-bezier(0.22, 1, 0.36, 1);
	}

	:global(::view-transition-old(settings-panel)),
	:global(::view-transition-new(settings-panel)) {
		animation-duration: 220ms;
	}

	@media (prefers-reduced-motion: reduce) {
		:global(::view-transition-group(settings-panel)),
		:global(::view-transition-old(settings-panel)),
		:global(::view-transition-new(settings-panel)) {
			animation-duration: 1ms;
		}
	}
</style>
