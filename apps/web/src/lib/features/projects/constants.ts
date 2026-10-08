/**
 * Used by the Imports form only until the workspace Settings default has loaded, or when Settings
 * cannot be read. It mirrors the server's workspace default, so the form never guesses another depth.
 */
export const IMPORT_FALLBACK_MAX_DEPTH = 1;
/**
 * Largest depth the Imports form offers by typing or stepping. A larger Settings default is still
 * shown as set.
 */
export const IMPORT_FORM_MAX_DEPTH = 4;
export const PROJECT_NAV_COLLAPSED_KEY = 'deepref:projects:nav-collapsed';
export const PROJECT_INSPECTOR_COLLAPSED_KEY = 'deepref:projects:inspector-collapsed';
export const PROJECT_WORKSPACE_MAIN_LAYOUT_ID = 'deepref-project-workspace-main';
export const PROJECT_WORKSPACE_INSPECTOR_LAYOUT_ID = 'deepref-project-workspace-inspector';
