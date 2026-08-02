export type ProjectWorkspaceView =
	'overview' | 'articles' | 'graph' | 'recommendations' | 'ingestions';

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
