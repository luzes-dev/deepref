import {
	uploadReportDocument,
	attachExternalReportDocument
} from '$lib/api/generated/documents/documents';
import type { ExternalDocumentRequest } from '$lib/api/generated/models';

export async function uploadPdf(projectId: string, reportId: string, file: File) {
	return uploadReportDocument(projectId, reportId, { file });
}

export async function attachExternalPdf(
	projectId: string,
	reportId: string,
	input: ExternalDocumentRequest
) {
	return attachExternalReportDocument(projectId, reportId, input);
}
