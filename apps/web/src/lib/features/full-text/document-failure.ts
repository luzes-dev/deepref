export type DocumentFailure = {
	/** One line for the researcher. */
	summary: string;
	/** Raw server text, shown under "Technical detail"; null when the summary says it all. */
	detail: string | null;
	/** False when trying the same link again cannot succeed (the link is wrong, not the network). */
	retryable: boolean;
};

/**
 * The API writes `document_link_rejected: <reason>` or `document_link_unavailable: <reason>`
 * when an external link failed. The reason is already written for the researcher.
 */
const LINK_FAILURE_CODE = /^(document_link_rejected|document_link_unavailable):\s*/;

/** Researcher-facing explanation for a document that failed processing. */
export function describeDocumentFailure(parserError: string | null | undefined): DocumentFailure {
	const detail = parserError?.trim() || null;
	const link = detail?.match(LINK_FAILURE_CODE);
	if (detail && link) {
		return {
			summary: detail.slice(link[0].length),
			detail: null,
			retryable: link[1] === 'document_link_unavailable'
		};
	}
	const text = detail?.toLowerCase() ?? '';
	if (isStoredPdfMissing(detail)) {
		return {
			summary: 'The stored PDF file is no longer on the server.',
			detail,
			retryable: true
		};
	}
	if (text.includes('pdfium') && (text.includes('load') || text.includes('library'))) {
		return {
			summary: "PDF processing isn't available on the server right now.",
			detail,
			retryable: true
		};
	}
	if (text.includes('password') || text.includes('encrypt')) {
		return {
			summary: 'This PDF is password-protected, so it could not be read.',
			detail,
			retryable: true
		};
	}
	if (text.includes('timed out') || text.includes('timeout')) {
		return { summary: 'Processing this PDF took too long.', detail, retryable: true };
	}
	return { summary: 'This PDF could not be processed.', detail, retryable: true };
}

/**
 * True when the stored PDF bytes are gone, so the file can only come back by
 * uploading it again. Matches the API's stable code and the raw object-store
 * message that older rows carry.
 */
export function isStoredPdfMissing(parserError: string | null | undefined): boolean {
	const text = parserError?.toLowerCase() ?? '';
	return (
		text.includes('document_blob_missing') ||
		(text.includes('object at location') && text.includes('not found'))
	);
}
