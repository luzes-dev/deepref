export type ReportLabelSource = {
	report_id: string;
	title?: string | null;
	doi?: string | null;
};

export function reportLabel(report: ReportLabelSource): string {
	return report.title ?? report.doi ?? report.report_id;
}

export function reportSearchText(report: ReportLabelSource): string {
	return `${report.title ?? ''} ${report.doi ?? ''} ${report.report_id}`.toLowerCase();
}
