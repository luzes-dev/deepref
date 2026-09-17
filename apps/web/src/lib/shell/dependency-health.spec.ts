import { describe, it, expect, vi } from 'vitest';
import {
	dependencyDetailSummary,
	dependencyLabel,
	observeDependencyHealth,
	type DependencyHealthCallbacks,
	type DependencyHealthObservation
} from './dependency-health';
import type { DependencyStatus } from '$lib/api/generated/models';

const allAvailable: DependencyStatus = {
	postgresql: { state: 'available' },
	worker: { state: 'available' }
};

function degradedWorker(): DependencyStatus {
	return {
		postgresql: { state: 'available' },
		worker: { state: 'degraded', backlog: 7 }
	};
}

function callbacks() {
	const onDegraded = vi.fn<DependencyHealthCallbacks['onDegraded']>();
	const onRecovered = vi.fn<DependencyHealthCallbacks['onRecovered']>();
	const onStatusError = vi.fn<DependencyHealthCallbacks['onStatusError']>();
	return { onDegraded, onRecovered, onStatusError };
}

const baseline: DependencyHealthObservation = { error: false, states: null };

describe('dependencyLabel', () => {
	it('capitalizes the dependency name', () => {
		expect.hasAssertions();
		expect(dependencyLabel('worker')).toBe('Worker');
	});
});

describe('dependencyDetailSummary', () => {
	it('appends lag and backlog to the state', () => {
		expect.hasAssertions();
		expect(dependencyDetailSummary({ state: 'degraded', backlog: 7 })).toBe(
			'degraded · backlog 7'
		);
		expect(dependencyDetailSummary({ state: 'available' })).toBe('available');
	});
});

describe('observeDependencyHealth', () => {
	it('announces degradation on first observation and stays quiet on repeat polls', () => {
		expect.hasAssertions();
		const spies = callbacks();

		const first = observeDependencyHealth(baseline, degradedWorker(), null, spies);
		expect(spies.onDegraded).toHaveBeenCalledTimes(1);
		expect(spies.onDegraded).toHaveBeenCalledWith('worker', 'degraded · backlog 7', false);
		expect(first.states).toEqual({ postgresql: 'available', worker: 'degraded' });

		const second = observeDependencyHealth(first, degradedWorker(), null, spies);
		expect(spies.onDegraded).toHaveBeenCalledTimes(1);
		expect(second.states).toEqual(first.states);
	});

	it('announces recovery once every dependency is available again', () => {
		expect.hasAssertions();
		const spies = callbacks();
		const previous: DependencyHealthObservation = {
			error: false,
			states: { postgresql: 'available', worker: 'degraded' }
		};

		const next = observeDependencyHealth(previous, allAvailable, null, spies);
		expect(spies.onRecovered).toHaveBeenCalledTimes(1);
		expect(spies.onRecovered).toHaveBeenCalledWith();
		expect(next.states).toEqual({ postgresql: 'available', worker: 'available' });
	});

	it('does not announce recovery on the very first observation', () => {
		expect.hasAssertions();
		const spies = callbacks();

		observeDependencyHealth(baseline, allAvailable, null, spies);
		expect(spies.onRecovered).not.toHaveBeenCalled();
		expect(spies.onDegraded).not.toHaveBeenCalled();
	});

	it('announces core interruption only when postgresql is unavailable', () => {
		expect.hasAssertions();
		const spies = callbacks();
		const status: DependencyStatus = {
			postgresql: { state: 'unavailable' },
			worker: { state: 'available' }
		};

		observeDependencyHealth(baseline, status, null, spies);
		expect(spies.onDegraded).toHaveBeenCalledWith('postgresql', 'unavailable', true);
	});

	it('announces fetch failures once and keeps the last known states', () => {
		expect.hasAssertions();
		const spies = callbacks();
		const previous: DependencyHealthObservation = {
			error: false,
			states: { postgresql: 'available', worker: 'degraded' }
		};
		const failure = new Error('health endpoint unreachable');

		const errored = observeDependencyHealth(previous, undefined, failure, spies);
		expect(spies.onStatusError).toHaveBeenCalledTimes(1);
		expect(spies.onStatusError).toHaveBeenCalledWith('health endpoint unreachable');
		expect(errored.states).toEqual(previous.states);

		observeDependencyHealth(errored, undefined, failure, spies);
		expect(spies.onStatusError).toHaveBeenCalledTimes(1);
	});

	it('announces recovery of a stale degradation after errors resolve', () => {
		expect.hasAssertions();
		const spies = callbacks();
		const errored: DependencyHealthObservation = {
			error: true,
			states: { postgresql: 'available', worker: 'degraded' }
		};

		const next = observeDependencyHealth(errored, allAvailable, null, spies);
		expect(spies.onRecovered).toHaveBeenCalledTimes(1);
		expect(next.error).toBe(false);
	});
});
