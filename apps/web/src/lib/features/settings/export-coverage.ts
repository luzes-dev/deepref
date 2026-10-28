/** Status manifests come from historical archive versions as well as new jobs. */
export function archiveCoverage(value: unknown) {
	const strings = (items: unknown): string[] =>
		Array.isArray(items)
			? items.filter((item): item is string => typeof item === 'string')
			: [];
	if (typeof value !== 'object' || value === null) return null;
	const sections = 'json_sections' in value ? strings(value.json_sections) : [];
	const exclusions = 'exclusions' in value ? strings(value.exclusions) : [];
	if (sections.length === 0) return null;
	return { sections, exclusions };
}
