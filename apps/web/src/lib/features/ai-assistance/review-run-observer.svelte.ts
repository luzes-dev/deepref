import { getReviewRun, retryReviewRun } from '#lib/api/generated/ai/ai.js';
import type { ReviewRunDto, ReviewRunStateDto } from '#lib/api/generated/models/index.js';
import { useInterval } from 'runed';

type TerminalCallback = (proposalId: string) => void | Promise<void>;

function runFailureMessage(state: ReviewRunStateDto): string | undefined {
	return state.kind === 'blocked' || state.kind === 'failed' ? state.message : undefined;
}

function completedProposalId(state: ReviewRunStateDto): string | undefined {
	return state.kind === 'completed' ? state.proposal_id : undefined;
}

function loadErrorMessage(error: unknown): string {
	return error instanceof Error ? error.message : 'Review run status could not be loaded.';
}

export class ReviewRunObserver {
	current = $state.raw<ReviewRunDto>();
	error = $state('');
	/** True while a retry or regeneration request is being sent. */
	restarting = $state(false);

	#runId: string | undefined;
	#refreshing = false;
	#settledRunId: string | undefined;
	#projectId: () => string;
	#onCompleted: TerminalCallback;
	#interval: ReturnType<typeof useInterval>;

	constructor(projectId: () => string, onCompleted: TerminalCallback) {
		this.#projectId = projectId;
		this.#onCompleted = onCompleted;
		this.#interval = useInterval(1_500, {
			immediate: false,
			callback: () => void this.refresh()
		});
	}

	get isActive(): boolean {
		return this.current?.state.kind === 'queued' || this.current?.state.kind === 'running';
	}

	get state(): ReviewRunStateDto | undefined {
		return this.current?.state;
	}

	/** A failed or blocked run can be retried as a new attempt. */
	get canRetry(): boolean {
		return this.current?.state.kind === 'failed' || this.current?.state.kind === 'blocked';
	}

	/**
	 * A completed run can be regenerated once its suggestion was rejected; the
	 * server refuses otherwise.
	 */
	get canRegenerate(): boolean {
		return this.current?.state.kind === 'completed';
	}

	/** Retries a failed or blocked run and observes the new attempt. */
	async retry(): Promise<void> {
		if (!this.canRetry) return;
		await this.#restart(false);
	}

	/** Asks for a fresh opinion after the completed run's suggestion was rejected. */
	async regenerate(): Promise<void> {
		if (!this.canRegenerate) return;
		await this.#restart(true);
	}

	async #restart(regenerate: boolean): Promise<void> {
		const runId = this.#runId;
		if (!runId || this.restarting) return;
		this.restarting = true;
		this.error = '';
		try {
			const response = await retryReviewRun(this.#projectId(), runId, { regenerate });
			await this.observe(response.data);
		} catch (error) {
			this.error = loadErrorMessage(error);
		} finally {
			this.restarting = false;
		}
	}

	async observe(run: ReviewRunDto): Promise<void> {
		this.current = run;
		this.#runId = run.id;
		this.#settledRunId = undefined;
		this.error = '';
		await this.#settleOrContinue(run);
		if (this.isActive) await this.refresh();
	}

	async observeId(runId: string): Promise<void> {
		this.current = undefined;
		this.#runId = runId;
		this.#settledRunId = undefined;
		this.error = '';
		this.#interval.resume();
		await this.refresh();
	}

	async refresh(): Promise<void> {
		if (!this.#runId || this.#refreshing) return;
		this.#refreshing = true;
		try {
			const response = await getReviewRun(this.#projectId(), this.#runId);
			this.current = response.data;
			this.error = '';
			await this.#settleOrContinue(response.data);
		} catch (error) {
			this.#interval.pause();
			this.error = loadErrorMessage(error);
		} finally {
			this.#refreshing = false;
		}
	}

	reset(): void {
		this.#interval.pause();
		this.current = undefined;
		this.#runId = undefined;
		this.#settledRunId = undefined;
		this.error = '';
	}

	async #settleOrContinue(run: ReviewRunDto): Promise<void> {
		const failureMessage = runFailureMessage(run.state);
		if (failureMessage !== undefined) {
			this.#interval.pause();
			this.error = failureMessage;
			return;
		}

		const proposalId = completedProposalId(run.state);
		if (proposalId === undefined) {
			this.#interval.resume();
			return;
		}

		this.#interval.pause();
		if (this.#settledRunId === run.id) return;
		this.#settledRunId = run.id;
		await this.#onCompleted(proposalId);
	}
}
