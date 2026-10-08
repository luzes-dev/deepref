import { describe, it, expect } from 'vitest';
import {
	decodeHtmlEntities,
	notificationHref,
	plainNotificationText,
	plainText
} from './notification-copy';

const PLAIN_AI_FAILURE =
	'The AI could not produce a valid suggestion (it did not follow the protocol). Try again or decide yourself.';

describe('decodeHtmlEntities', () => {
	it('decodes named and numeric entities once', () => {
		expect.hasAssertions();
		expect(decodeHtmlEntities('Lancet Diabetes &amp; Endocrinology')).toBe(
			'Lancet Diabetes & Endocrinology'
		);
		expect(decodeHtmlEntities('&lt;b&gt; &quot;quoted&quot; &#39;x&#39;')).toBe(
			'<b> "quoted" \'x\''
		);
		expect(decodeHtmlEntities('Trial&#x2019;s result &#8211; 2016')).toBe(
			'Trial’s result – 2016'
		);
	});

	it('does not decode a double-encoded ampersand a second time', () => {
		expect.hasAssertions();
		expect(decodeHtmlEntities('AT&amp;amp;T')).toBe('AT&amp;T');
	});

	it('leaves unknown or invalid entities as they are written', () => {
		expect.hasAssertions();
		expect(decodeHtmlEntities('&unknown; &#0; &#x110000;')).toBe('&unknown; &#0; &#x110000;');
		expect(decodeHtmlEntities('plain text')).toBe('plain text');
	});
});

describe('plainText', () => {
	it('translates the review-execution jargon and keeps the original behind a disclosure', () => {
		expect.hasAssertions();
		const raw = 'review execution failed: AI output failed semantic validation';
		expect(plainText(raw)).toEqual({ text: PLAIN_AI_FAILURE, technical: raw });
	});

	it('translates the structured-output wording the same way', () => {
		expect.hasAssertions();
		const raw = 'review execution failed: structured output failed semantic validation';
		expect(plainText(raw).text).toBe(PLAIN_AI_FAILURE);
	});

	it('says plainly when the AI provider has run out of credit', () => {
		expect.hasAssertions();
		const raw =
			'review execution failed: AI provider is unavailable: provider account has insufficient balance';
		expect(plainText(raw)).toEqual({
			text: 'The AI provider account has run out of credit. Top it up, then try again; nothing was lost.',
			technical: raw
		});
	});

	it('says plainly when the AI subscription limit is reached', () => {
		expect.hasAssertions();
		const raw = 'review execution failed: AI subscription limit reached; try again later';
		expect(plainText(raw)).toEqual({
			text: 'The AI subscription has reached its usage limit for now. Try again later; nothing was lost.',
			technical: raw
		});
	});

	it('translates persistence failures into plain language', () => {
		expect.hasAssertions();
		expect(plainText('proposal persistence or validation failed').text).toBe(
			"The AI's suggestion could not be saved. Try again."
		);
		expect(plainText('AI run persistence failed').text).toBe(
			"The AI's answer could not be saved. Try again."
		);
	});

	it('shows wording it does not know unchanged, with no disclosure', () => {
		expect.hasAssertions();
		expect(plainText('  Recent papers finished &amp; saved.  ')).toEqual({
			text: 'Recent papers finished & saved.',
			technical: null
		});
	});
});

describe('plainNotificationText', () => {
	it('decodes the title, translates the body and collects the original wording', () => {
		expect.hasAssertions();
		const copy = plainNotificationText({
			title: 'Included: Lancet Diabetes &amp; Endocrinology',
			body: 'review execution failed: AI output failed semantic validation'
		});
		expect(copy.title).toBe('Included: Lancet Diabetes & Endocrinology');
		expect(copy.body).toBe(PLAIN_AI_FAILURE);
		expect(copy.technical).toBe(
			'review execution failed: AI output failed semantic validation'
		);
	});

	it('treats a blank body as no body', () => {
		expect.hasAssertions();
		expect(plainNotificationText({ title: 'Import completed', body: '   ' })).toEqual({
			title: 'Import completed',
			body: null,
			technical: null
		});
	});
});

describe('notificationHref', () => {
	const projectId = '33438758-42f0-4da2-9c3d-22a2c51f1109';
	const base = `/projects/${projectId}`;

	it('opens the builder at the run for workflow notifications', () => {
		expect.hasAssertions();
		expect(
			notificationHref({
				kind: 'workflow_run.failed',
				project_id: projectId,
				payload: { workflow_id: 'wf-1', run_id: 'run-9' }
			})
		).toBe(`${base}/automations/wf-1?run=run-9`);
		expect(
			notificationHref({
				kind: 'workflow',
				project_id: projectId,
				payload: { workflow_id: 'wf-1' }
			})
		).toBe(`${base}/automations/wf-1`);
	});

	it('falls back to the automations list when the workflow is unknown', () => {
		expect.hasAssertions();
		expect(notificationHref({ kind: 'workflow', project_id: projectId, payload: {} })).toBe(
			`${base}/automations`
		);
	});

	it('links imports to the ingestion they describe', () => {
		expect.hasAssertions();
		expect(
			notificationHref({
				kind: 'acquisition.failed',
				project_id: projectId,
				payload: { acquisition_id: 'acq 1' }
			})
		).toBe(`${base}/discovery/imports?ingestion=acq%201`);
	});

	it('sends failed AI runs to AI activity and finished proposals to recommendations', () => {
		expect.hasAssertions();
		expect(
			notificationHref({ kind: 'review_run.failed', project_id: projectId, payload: {} })
		).toBe(`${base}/activity`);
		expect(
			notificationHref({ kind: 'review_run.completed', project_id: projectId, payload: {} })
		).toBe(`${base}/recommendations`);
	});

	it('has no link without a project or for a kind it does not know', () => {
		expect.hasAssertions();
		expect(notificationHref({ kind: 'workflow', project_id: null, payload: {} })).toBeNull();
		expect(notificationHref({ kind: 'something.else', project_id: projectId })).toBeNull();
	});
});
