import type { DependencyStatus } from '$lib/api/generated/models';

export interface DependencyHealthObservation {
	error: boolean;
	states: Record<string, string> | null;
}

export interface DependencyHealthCallbacks {
	onDegraded: (name: string, summary: string, coreUnavailable: boolean) => void;
	onRecovered: () => void;
	onStatusError: (message: string) => void;
}

export function dependencyLabel(name: string): string {
	return name.charAt(0).toUpperCase() + name.slice(1);
}

export function dependencyDetailSummary(detail: {
	state: string;
	lag?: number | null;
	backlog?: number | null;
}): string {
	let summary = detail.state;
	if (detail.lag) summary += ` · lag ${detail.lag}`;
	if (detail.backlog) summary += ` · backlog ${detail.backlog}`;
	return summary;
}

export function observeDependencyHealth(
	previous: DependencyHealthObservation,
	status: DependencyStatus | undefined,
	error: Error | null | undefined,
	callbacks: DependencyHealthCallbacks
): DependencyHealthObservation {
	if (error) {
		if (!previous.error) callbacks.onStatusError(error.message);
		return { error: true, states: previous.states };
	}
	if (!status) return previous;

	const states: Record<string, string> = {};
	for (const [name, detail] of Object.entries(status)) states[name] = detail.state;

	let hasDegraded = false;
	for (const [name, detail] of Object.entries(status)) {
		if (detail.state === 'available') continue;
		hasDegraded = true;
		if (previous.states === null || previous.states[name] === 'available') {
			callbacks.onDegraded(
				name,
				dependencyDetailSummary(detail),
				name === 'postgresql' && detail.state === 'unavailable'
			);
		}
	}

	const previouslyDegraded =
		previous.states !== null &&
		Object.values(previous.states).some((state) => state !== 'available');
	if (previouslyDegraded && !hasDegraded) callbacks.onRecovered();

	return { error: false, states };
}
