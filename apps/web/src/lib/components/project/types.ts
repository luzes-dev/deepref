export const PROJECTS_ROUTE = '/';
export const PROJECT_PARAM = 'project';
export const VIEW_PARAM = 'view';
export const ARTICLE_PARAM = 'article';
export const INGESTION_PARAM = 'ingestion';

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
