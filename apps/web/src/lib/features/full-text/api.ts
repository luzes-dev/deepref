import {
	uploadReportDocument,
	reparseReportDocument,
	attachExternalReportDocument,
	attachOpenAccessReportDocument,
	acknowledgeReportDocumentIdentity,
	deleteReportDocument
} from '#lib/api/generated/documents/documents.js';
import type { ExternalDocumentRequest } from '#lib/api/generated/models/index.js';

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

/** Looks up an open-access PDF by the report's DOI and attaches it as an external link. */
export async function attachOpenAccessPdf(projectId: string, reportId: string) {
	return attachOpenAccessReportDocument(projectId, reportId);
}

export async function retryDocumentProcessing(
	projectId: string,
	reportId: string,
	documentId: string
) {
	return reparseReportDocument(projectId, reportId, documentId);
}

/** Keeps a PDF that was flagged as possibly belonging to another article. */
export async function keepFlaggedPdf(projectId: string, reportId: string, documentId: string) {
	return acknowledgeReportDocumentIdentity(projectId, reportId, documentId);
}

export async function removeDocument(projectId: string, reportId: string, documentId: string) {
	return deleteReportDocument(projectId, reportId, documentId);
}
