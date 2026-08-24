export type ProjectWorkspaceView =
	| 'overview'
	| 'protocol'
	| 'articles'
	| 'graph'
	| 'recommendations'
	| 'ingestions'
	| 'duplicates'
	| 'screening';

export type ProjectWorkspaceNavView = Exclude<ProjectWorkspaceView, 'duplicates' | 'screening'>;

export type ProjectWorkspaceCounts = {
	articles?: number;
	recommendations?: number;
	ingestions?: number;
};
