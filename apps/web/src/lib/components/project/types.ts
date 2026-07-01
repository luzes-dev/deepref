export const PROJECT_WORKSPACE_VIEWS = [
	'overview',
	'articles',
	'graph',
	'recommendations',
	'ingestions'
] as const;

export type ProjectWorkspaceView = (typeof PROJECT_WORKSPACE_VIEWS)[number];

export type ProjectWorkspaceState = {
	project?: string;
	view: ProjectWorkspaceView;
	article?: string;
	ingestion?: string;
};

export type ProjectWorkspaceCounts = {
	articles?: number;
	recommendations?: number;
	ingestions?: number;
};
