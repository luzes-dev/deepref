import type { Component } from "svelte";

/**
 * Callbacks the builder canvas hands to its block nodes. They live in context
 * rather than in node data because node data is copied by Svelte Flow, and
 * functions and components cannot be copied.
 */
export const BUILDER_CONTEXT = Symbol("deepref-builder-canvas");

export interface BuilderContext {
	/** Icon component for a block, looked up by the node's `iconKey`. */
	iconFor: (key: string) => Component<{ class?: string }> | undefined;
	/** Opens the "add a block" menu from an output port's "+" button. */
	addFrom: (nodeId: string, portId: string, anchor: HTMLElement) => void;
}
