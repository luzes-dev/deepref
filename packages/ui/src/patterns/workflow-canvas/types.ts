import type { Node, Edge } from "@xyflow/svelte";

export type FlowNodeKind =
	"trigger" | "action" | "agent" | "condition" | "human_gate";

export interface TriggerNodeData extends Record<string, unknown> {
	label: string;
	description?: string;
	triggerKey: string;
	status?: "active" | "paused" | "running" | "idle";
	icon?: string;
	isEventTrigger?: boolean;
}

export interface ActionNodeData extends Record<string, unknown> {
	label: string;
	description?: string;
	actionKey: string;
	kind?: string;
	status?: "queued" | "running" | "completed" | "failed" | "idle";
	duration?: string;
	attempts?: number;
}

export interface AgentNodeData extends Record<string, unknown> {
	label: string;
	description?: string;
	toolName: string;
	kind: "read" | "proposal" | "agent";
	authorityTier?: string;
	status?: "ready" | "running" | "completed" | "failed" | "idle";
	model?: string;
	temperature?: number;
}

export interface ConditionNodeData extends Record<string, unknown> {
	label: string;
	expression?: string;
	trueLabel?: string;
	falseLabel?: string;
}

export interface HumanGateNodeData extends Record<string, unknown> {
	label: string;
	description?: string;
	assignee?: string;
	status?: "pending" | "approved" | "rejected";
}

export type AnyFlowNode = Node<
	| TriggerNodeData
	| ActionNodeData
	| AgentNodeData
	| ConditionNodeData
	| HumanGateNodeData
>;

export type FlowEdge = Edge;

export type BuilderTone =
	"trigger" | "data" | "action" | "ai" | "logic" | "integration" | "note";

export type BuilderRunStatus =
	"queued" | "running" | "completed" | "failed" | "skipped" | "cancelled";

export interface BuilderPort {
	id: string;
	label: string;
	/** Name of the kind of data the port carries; used for the tooltip only. */
	type: string;
}

export interface BuilderNodeData extends Record<string, unknown> {
	title: string;
	/** Short name of the block kind, shown above the title. */
	kind?: string;
	/** One line describing the block's settings in plain words. */
	summary?: string;
	tone: BuilderTone;
	/** Key used to look up the block's icon (see the canvas `iconFor` prop). */
	iconKey?: string;
	inputs: BuilderPort[];
	outputs: BuilderPort[];
	/** Output port ids that already have a connection. */
	connectedOutputs?: string[];
	/** Problems found by validation, in plain words. */
	issues?: string[];
	status?: BuilderRunStatus;
	/** What the block did in the last test or run, e.g. "12 in, 9 yes / 3 no". */
	preview?: string;
	/** True for blocks a test run only pretends to run. */
	dryRun?: boolean;
	/** Shows the "+" button next to each free output, to add a block after it. */
	addable?: boolean;
}

export interface BuilderCanvasApi {
	screenToFlow: (point: { x: number; y: number }) => { x: number; y: number };
	fitView: () => void;
	getZoom: () => number;
	/** Pans the canvas, if needed, so a block sits inside the given visible area. */
	revealNode: (id: string, area: { width: number; height: number }) => void;
}
