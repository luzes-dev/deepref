<script lang="ts">
	import {
		manualStepPath,
		reviewQueuePathForTool,
		type AssistantPlan,
		type AssistantPlanAction,
		type AssistantPlanDeselection,
		type AssistantPlanResult
	} from '../chat-api';
	import * as Alert from '@deepref/ui/alert';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import * as Card from '@deepref/ui/card';
	import { Spinner } from '@deepref/ui/spinner';

	let {
		projectId,
		plan,
		busy = false,
		error = null,
		onconfirm,
		oncancel
	}: {
		projectId: string;
		plan: AssistantPlan;
		busy?: boolean;
		error?: string | null;
		onconfirm: (deselected: AssistantPlanDeselection[]) => void;
		oncancel: () => void;
	} = $props();

	let deselected = $state<AssistantPlanDeselection[]>([]);
	const hasExecutable = $derived(plan.actions.some((action) => action.executable));
	const hasSelected = $derived(plan.actions.some((action) =>
		action.executable && (action.tool !== 'screen_reports' ||
			action.items.some((item) => !isDeselected(action.id, item.report_id)))
	));

	function isDeselected(actionId: string, reportId: string): boolean {
		return deselected.some((item) => item.action_id === actionId && item.report_id === reportId);
	}

	function toggleRecord(actionId: string, reportId: string): void {
		deselected = isDeselected(actionId, reportId)
			? deselected.filter((item) => item.action_id !== actionId || item.report_id !== reportId)
			: [...deselected, { action_id: actionId, report_id: reportId }];
	}

	function headline(action: AssistantPlanAction): string {
		if (action.tool !== 'screen_reports') return action.summary;
		const count = action.items.filter((item) => !isDeselected(action.id, item.report_id)).length;
		const decision = action.decision ? action.decision[0].toUpperCase() + action.decision.slice(1) : 'Screen';
		return count === action.items.length
			? `${decision} ${count} ${count === 1 ? 'record' : 'records'}`
			: `${decision} ${count} of ${action.items.length} records`;
	}

	function statusText(status: string | null | undefined): string {
		switch (status) {
			case 'include': return 'Included';
			case 'exclude': return 'Excluded';
			case 'maybe': return 'Maybe';
			case 'unscreened': return 'Unscreened';
			default: return status ?? 'Unknown status';
		}
	}
	// An AI review that is still running is not "Done" until it finishes.
	const reviewPending = $derived(
		plan.results?.some((result) => result.status === 'queued') ?? false
	);
	const statusLabel = $derived.by(() => {
		if (plan.status === 'executed' && reviewPending) return 'Queued for review';
		switch (plan.status) {
			case 'pending': return 'Waiting for your OK';
			case 'confirmed': return 'Running';
			case 'rejected': return 'Cancelled';
			case 'executed': return 'Done';
			case 'failed': return 'Some steps failed';
			default: return plan.status;
		}
	});
	const statusVariant = $derived(
		plan.status === 'executed' && reviewPending
			? 'info'
			: plan.status === 'executed'
				? 'success'
				: plan.status === 'failed'
					? 'destructive'
					: plan.status === 'pending'
						? 'info'
						: 'secondary'
	);

	function resultFor(action: AssistantPlanAction): AssistantPlanResult | null {
		return plan.results?.find((result) => result.action_id === action.id) ?? null;
	}

	function countLabel(action: AssistantPlanAction): string {
		const count = action.tool === 'screen_reports' ? action.items.length : action.affected_count;
		return count === 1 ? '1 item' : `${count} items`;
	}
</script>

<Card.Root class="w-full" data-testid="assistant-plan" data-status={plan.status}>
	<Card.Header>
		<Card.Title>Proposed changes</Card.Title>
		<Card.Description>
			{#if plan.status === 'pending'}
				Nothing has been changed yet. Review the list, then confirm or cancel.
			{:else if plan.status === 'rejected'}
				You cancelled this plan. Nothing was changed.
			{:else if plan.status === 'confirmed'}
				Carrying out the plan…
			{:else if plan.status === 'failed'}
				Some steps did not finish. Details are under each step.
			{:else if reviewPending}
				The AI review runs in the background. Its result appears here when it finishes.
			{:else}
				This plan was carried out with your confirmation.
			{/if}
		</Card.Description>
		<Card.Action>
			<Badge variant={statusVariant} size="sm">{statusLabel}</Badge>
		</Card.Action>
	</Card.Header>
	<Card.Content class="flex flex-col gap-3">
		<ol class="flex flex-col gap-3">
			{#each plan.actions as action (action.id)}
				{@const result = resultFor(action)}
				<li class="flex flex-col gap-1 border-l pl-3" data-testid="assistant-plan-action">
					<div class="flex flex-wrap items-center gap-2">
						<span class="text-sm font-medium text-foreground" data-testid="assistant-plan-headline">{headline(action)}</span>
						{#if action.executable}
							<Badge variant="outline" size="xs">{countLabel(action)}</Badge>
						{/if}
					</div>
					{#if action.rationale}
						<p class="text-xs text-muted-foreground">{action.rationale}</p>
					{/if}
					{#if action.tool === 'screen_reports'}
						<ul class="flex flex-col gap-2">
							{#each action.items as item (item.report_id)}
								<li data-testid="assistant-plan-item">
									<label class="flex items-start gap-2 text-sm">
										{#if plan.status === 'pending' && action.executable}
											<input
												type="checkbox"
												checked={!isDeselected(action.id, item.report_id)}
												onchange={() => toggleRecord(action.id, item.report_id)}
												disabled={busy}
												aria-label={item.title}
											/>
										{/if}
										<span>
											{item.title}
											<span class="block text-xs text-muted-foreground">
												{statusText(item.current_status)} → {action.decision ? action.decision[0].toUpperCase() + action.decision.slice(1) : 'Screen'}
											</span>
										</span>
									</label>
								</li>
							{/each}
						</ul>
					{/if}
					{#if action.manual}
						<Alert.Root variant="warning">
							<Alert.Title>You need to do this yourself</Alert.Title>
							<Alert.Description>
								{action.manual.reason}
								<Button
									class="mt-2"
									size="sm"
									variant="outline"
									href={manualStepPath(projectId, action.manual.link_target)}
								>
									Open the page
								</Button>
							</Alert.Description>
						</Alert.Root>
					{/if}
					{#if result && !action.manual}
						<p
							class="text-xs {result.status === 'failed'
								? 'text-destructive'
								: 'text-muted-foreground'}"
							data-testid="assistant-plan-result"
						>
							{result.message}
							{#if result.review_run_id && result.status !== 'failed'}
								<Button
									variant="link"
									size="sm"
									href={reviewQueuePathForTool(action.tool, projectId) ??
										`/projects/${projectId}`}>Open review queue</Button
								>
							{/if}
						</p>
					{/if}
				</li>
			{/each}
		</ol>
		{#if error}
			<Alert.Root variant="destructive" role="alert">
				<Alert.Title>The plan could not be completed</Alert.Title>
				<Alert.Description>{error}</Alert.Description>
			</Alert.Root>
		{/if}
	</Card.Content>
	{#if plan.status === 'pending'}
		<Card.Footer class="justify-end gap-2">
			<Button variant="ghost" size="sm" onclick={oncancel} disabled={busy}>Cancel</Button>
			{#if hasExecutable}
				<Button
					size="sm"
					onclick={() => onconfirm(deselected)}
					disabled={busy || !hasSelected}
					data-testid="assistant-plan-confirm"
				>
					{#if busy}<Spinner />{/if}
					Confirm
				</Button>
			{/if}
		</Card.Footer>
	{/if}
</Card.Root>
