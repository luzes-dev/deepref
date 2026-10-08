import { describe, it, expect } from 'vitest';
import { dependencyLabel, summarizeDependencyHealth } from './dependency-health';
import type { DependencyStatus } from '$lib/api/generated/models';

const allAvailable: DependencyStatus = {
	postgresql: { state: 'available' },
	worker: { state: 'available' }
};

describe('dependencyLabel', () => {
	it('uses researcher-facing names', () => {
		expect.hasAssertions();
		expect(dependencyLabel('worker')).toBe('Background jobs');
		expect(dependencyLabel('postgresql')).toBe('Database');
		expect(dependencyLabel('cache')).toBe('Cache');
	});
});

describe('summarizeDependencyHealth', () => {
	it('is silent while everything is available or nothing has loaded yet', () => {
		expect.hasAssertions();
		expect(summarizeDependencyHealth(allAvailable, null)).toEqual({ level: 'ok', issues: [] });
		expect(summarizeDependencyHealth(undefined, null).level).toBe('ok');
	});

	it('reports a degraded worker with its backlog', () => {
		expect.hasAssertions();
		const summary = summarizeDependencyHealth(
			{ ...allAvailable, worker: { state: 'degraded', backlog: 7 } },
			null
		);
		expect(summary.level).toBe('degraded');
		expect(summary.issues).toHaveLength(1);
		expect(summary.issues[0]).toMatchObject({ name: 'worker', label: 'Background jobs' });
		expect(summary.issues[0].message).toContain('7 jobs are waiting');
	});

	it('escalates to unavailable when the database is down', () => {
		expect.hasAssertions();
		const summary = summarizeDependencyHealth(
			{ postgresql: { state: 'unavailable' }, worker: { state: 'degraded' } },
			null
		);
		expect(summary.level).toBe('unavailable');
		expect(summary.issues).toHaveLength(2);
	});

	it('reports an unreachable status endpoint and clears once it answers again', () => {
		expect.hasAssertions();
		const failed = summarizeDependencyHealth(undefined, new Error('boom'));
		expect(failed.level).toBe('unavailable');
		expect(failed.issues[0].name).toBe('status');
		expect(summarizeDependencyHealth(allAvailable, null).level).toBe('ok');
	});

	it('says how many jobs failed in this project when the probe is project-scoped', () => {
		expect.hasAssertions();
		const summary = summarizeDependencyHealth(
			{ ...allAvailable, worker: { state: 'degraded', backlog: 0, recent_failed: 2 } },
			null,
			{ scope: 'project' }
		);
		expect(summary.level).toBe('degraded');
		expect(summary.issues[0].message).toBe(
			'2 background jobs failed in the last 30 minutes in this project. Imports and automations may be incomplete.'
		);
	});

	it('uses singular wording for one failed job and names the workspace for an unscoped probe', () => {
		expect.hasAssertions();
		const summary = summarizeDependencyHealth(
			{ ...allAvailable, worker: { state: 'degraded', backlog: null, recent_failed: 1 } },
			null
		);
		expect(summary.issues[0].message).toContain(
			'1 background job failed in the last 30 minutes in the workspace.'
		);
	});

	it('stays silent for a clean project even when the workspace has failures elsewhere', () => {
		expect.hasAssertions();
		const summary = summarizeDependencyHealth(
			{ ...allAvailable, worker: { state: 'available', backlog: 0, recent_failed: 0 } },
			null,
			{ scope: 'project' }
		);
		expect(summary).toEqual({ level: 'ok', issues: [] });
	});
});
