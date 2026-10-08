import { describe, expect, it } from 'vitest';
import { describeDocumentFailure, isStoredPdfMissing } from './document-failure';

describe('describeDocumentFailure', () => {
	it('says the stored file is gone when the object store cannot find it', () => {
		const raw =
			'document object store operation failed: Object at location /tmp/deepref-documents/documents/ea00 not found: No such file or directory (os error 2)';
		const result = describeDocumentFailure(raw);
		expect(result.summary).toBe('The stored PDF file is no longer on the server.');
		expect(result.detail).toBe(raw);
	});

	it('recognises the stable blob-missing code from the API', () => {
		expect(
			describeDocumentFailure(
				'document_blob_missing: the stored PDF file is no longer available; upload the PDF again'
			).summary
		).toBe('The stored PDF file is no longer on the server.');
		expect(isStoredPdfMissing('document_blob_missing: upload again')).toBe(true);
		expect(isStoredPdfMissing('document object store operation failed: timed out')).toBe(false);
	});

	it('explains a missing pdfium library without exposing internals as the summary', () => {
		const result = describeDocumentFailure(
			'Pdfium could not be loaded: libpdfium.so not found'
		);
		expect(result.summary).toBe("PDF processing isn't available on the server right now.");
		expect(result.detail).toContain('libpdfium');
	});

	it('falls back to a generic line and keeps the raw detail', () => {
		const result = describeDocumentFailure('unexpected EOF');
		expect(result.summary).toBe('This PDF could not be processed.');
		expect(result.detail).toBe('unexpected EOF');
	});

	it('handles a missing error', () => {
		expect(describeDocumentFailure(null).detail).toBeNull();
	});
});

describe('describeDocumentFailure for external links', () => {
	it('shows the reason as the summary and does not offer a retry for a rejected link', () => {
		const result = describeDocumentFailure(
			'document_link_rejected: The link returned 404 \u2013 not found'
		);
		expect(result).toEqual({
			summary: 'The link returned 404 \u2013 not found',
			detail: null,
			retryable: false
		});
	});

	it('keeps retry available when the link was only temporarily unavailable', () => {
		const result = describeDocumentFailure(
			'document_link_unavailable: The link returned 503 \u2013 service unavailable'
		);
		expect(result.summary).toBe('The link returned 503 \u2013 service unavailable');
		expect(result.retryable).toBe(true);
	});

	it('explains a web page instead of a PDF', () => {
		expect(
			describeDocumentFailure('document_link_rejected: The link is a web page, not a PDF')
				.summary
		).toBe('The link is a web page, not a PDF');
	});

	it('treats ordinary processing failures as retryable', () => {
		expect(describeDocumentFailure('unexpected EOF').retryable).toBe(true);
	});
});
