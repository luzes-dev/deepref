import { describe, expect, it } from 'vitest';
import { parseSseFrame, splitSseFrames, AssistantStreamError } from '$lib/api/assistant-stream';
import {
	reviewDestinationForTool,
	reviewQueuePathForTool,
	deriveConversationTitle
} from './chat-api';

describe('splitSseFrames', () => {
	it('splits complete frames and keeps the remainder buffered', () => {
		const raw =
			'event: token\ndata: {"delta":"Hi"}\n\nevent: token\ndata: {"delta":" there"}\n\nevent: tok';
		const { frames, remainder } = splitSseFrames(raw);
		expect(frames).toHaveLength(2);
		expect(remainder).toBe('event: tok');
	});

	it('normalizes crlf frame breaks', () => {
		const { frames, remainder } = splitSseFrames('event: token\r\ndata: {"delta":"a"}\r\n\r\n');
		expect(frames).toHaveLength(1);
		expect(remainder).toBe('');
	});

	it('drops blank frames but keeps the remainder untouched', () => {
		const { frames, remainder } = splitSseFrames('\n\n: keepalive\n\n');
		expect(frames).toHaveLength(1);
		expect(remainder).toBe('');
	});
});

describe('parseSseFrame', () => {
	it('parses token events', () => {
		expect(parseSseFrame('event: token\ndata: {"delta":"hi"}')).toEqual({
			event: 'token',
			delta: 'hi'
		});
	});

	it('parses tool lifecycle events with args and output', () => {
		expect(
			parseSseFrame(
				'event: tool_start\ndata: {"tool":"get_report","tool_call_id":"c1","args":{"report_id":"r1"}}'
			)
		).toEqual({
			event: 'tool_start',
			tool: 'get_report',
			tool_call_id: 'c1',
			args: { report_id: 'r1' }
		});

		expect(
			parseSseFrame(
				'event: tool_complete\ndata: {"tool":"get_report","tool_call_id":"c1","output":{"title":"T"}}'
			)
		).toEqual({
			event: 'tool_complete',
			tool: 'get_report',
			tool_call_id: 'c1',
			output: { title: 'T' }
		});
	});

	it('parses proposal and done events', () => {
		expect(
			parseSseFrame(
				'event: proposal_created\ndata: {"tool":"propose_screening_decision","review_run_id":"rr1","status_path":"/p"}'
			)
		).toEqual({
			event: 'proposal_created',
			tool: 'propose_screening_decision',
			review_run_id: 'rr1',
			status_path: '/p'
		});

		expect(
			parseSseFrame(
				'event: done\ndata: {"message_id":"m1","input_tokens":10,"output_tokens":5}'
			)
		).toEqual({
			event: 'done',
			message_id: 'm1',
			input_tokens: 10,
			output_tokens: 5
		});
	});

	it('returns null for unknown events and malformed payloads', () => {
		expect(parseSseFrame('event: mystery\ndata: {"x":1}')).toBeNull();
		expect(parseSseFrame('event: token\ndata: not-json')).toBeNull();
		expect(parseSseFrame('data: {"delta":"orphan"}')).toBeNull();
		expect(parseSseFrame(': comment only')).toBeNull();
	});

	it('coerces malformed fields defensively', () => {
		expect(parseSseFrame('event: token\ndata: {}')).toEqual({ event: 'token', delta: '' });
		expect(parseSseFrame('event: error\ndata: {"message":"boom"}')).toEqual({
			event: 'error',
			message: 'boom'
		});
	});
});

describe('AssistantStreamError', () => {
	it('carries the http status', () => {
		const error = new AssistantStreamError(403, 'forbidden');
		expect(error.status).toBe(403);
		expect(error.message).toBe('forbidden');
		expect(error.name).toBe('AssistantStreamError');
	});
});

describe('reviewDestinationForTool', () => {
	it('maps every proposal tool to its review queue', () => {
		expect(reviewDestinationForTool('propose_screening_decision')).toBe('screening');
		expect(reviewDestinationForTool('propose_duplicate_merge')).toBe('deduplication');
		expect(reviewDestinationForTool('propose_study_grouping')).toBe('studies');
		expect(reviewDestinationForTool('propose_classification')).toBe('studies');
		expect(reviewDestinationForTool('propose_extraction')).toBe('extraction');
		expect(reviewDestinationForTool('propose_appraisal_answer')).toBe('appraisal');
		expect(reviewDestinationForTool('get_report')).toBeNull();
		expect(reviewDestinationForTool('unknown_tool')).toBeNull();
	});

	it('builds review queue paths scoped to the project', () => {
		expect(reviewQueuePathForTool('propose_screening_decision', 'p1')).toBe(
			'/projects/p1/screening'
		);
		expect(reviewQueuePathForTool('get_report', 'p1')).toBeNull();
	});
});

describe('deriveConversationTitle', () => {
	it('uses the trimmed message for short prompts', () => {
		expect(deriveConversationTitle('  What is in the protocol?  ')).toBe(
			'What is in the protocol?'
		);
	});

	it('falls back to a default for blank prompts', () => {
		expect(deriveConversationTitle('   ')).toBe('New chat');
	});

	it('truncates long prompts at a word boundary', () => {
		const title = deriveConversationTitle(
			'screen report for inclusion criteria and explain the rationale behind the decision'
		);
		expect(title.length).toBeLessThanOrEqual(61);
		expect(title.endsWith('…')).toBe(true);
	});
});
