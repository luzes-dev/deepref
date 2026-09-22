// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		interface PageState {
			/** The route that remains mounted while Settings is shown as a shallow overlay. */
			settingsOverlay?: {
				backgroundUrl: string;
			};
			/** Marks the full-page Settings entry created by expanding the overlay. */
			settingsExpansion?: {
				backgroundUrl: string;
			};
			/** Full-text's transient search state survives a shallow Settings overlay. */
			deeprefFullTextSearch?: string;
		}
		// interface Platform {}
	}
}

export {};
