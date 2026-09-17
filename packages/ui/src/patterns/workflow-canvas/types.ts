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
