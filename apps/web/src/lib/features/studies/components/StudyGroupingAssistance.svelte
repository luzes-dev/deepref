<script lang="ts">
	import type {
		AiProposalDecisionInput,
		AiProposalDto,
		AiStudyGroupingEvidenceDto,
		AiStudyGroupingFieldDto,
		AiStudyGroupingProposalPayload,
		StudyMembershipDto
	} from '$lib/api/generated/models';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Brain, Check, Info, X } from '@lucide/svelte';

	let {
		reportId,
		proposal,
		payload,
		membership,
		pending,
		errorMessage,
		conflict,
		providerUnavailable,
		action,
		decisionPending,
		studyLabel,
		onGenerate,
		onDecide
	}: {
		reportId: string;
		proposal: AiProposalDto | undefined;
		payload: AiStudyGroupingProposalPayload | undefined;
		membership: StudyMembershipDto | undefined;
		pending: boolean;
		errorMessage: string;
		conflict: boolean;
		providerUnavailable: boolean;
		action: 'generate' | 'accept' | 'reject' | null;
		decisionPending: boolean;
		studyLabel: (studyId: string) => string;
		onGenerate: () => void;
		onDecide: (decision: AiProposalDecisionInput) => void;
	} = $props();

	function fieldLabel(field: AiStudyGroupingFieldDto): string {
		const labels = {
			title: 'Title',
			abstract: 'Abstract',
			publication_year: 'Publication year',
			first_author: 'First author'
		} satisfies Record<AiStudyGroupingFieldDto, string>;
		return labels[field];
	}

	function evidenceSubject(evidence: AiStudyGroupingEvidenceDto): string {
		switch (evidence.kind) {
			case 'report_metadata':
				return `Report ${evidence.report_id}`;
			case 'study_metadata':
				return `Study ${studyLabel(evidence.study_id)}`;
			case 'study_report_metadata':
				return `Study ${studyLabel(evidence.study_id)} · report ${evidence.report_id}`;
		}
	}
</script>

<Card.Root data-testid="study-grouping-assistance">
	<Card.Header class="gap-3">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<div class="flex items-center gap-2">
				<Brain aria-hidden="true" class="size-4" />
				<Card.Title>Study grouping assistance</Card.Title>
			</div>
			<Badge variant="outline">Proposal only</Badge>
		</div>
		<Card.Description>
			AI compares report metadata with existing studies and proposes a reversible grouping. A
			reviewer must approve it; grouping never changes screening or appraisal decisions.
		</Card.Description>
	</Card.Header>
	<Card.Content class="flex flex-col gap-4">
		{#if errorMessage}
			<Alert.Root variant="destructive" role="alert">
				<Alert.Title>
					{conflict
						? 'Study data changed elsewhere'
						: providerUnavailable
							? 'AI provider unavailable'
							: 'Study grouping suggestion unavailable'}
				</Alert.Title>
				<Alert.Description>{errorMessage}</Alert.Description>
			</Alert.Root>
		{:else if !reportId}
			<Empty.Root class="border-0 p-0">
				<Empty.Media variant="icon"><Info /></Empty.Media>
				<Empty.Header>
					<Empty.Title>Select a report</Empty.Title>
					<Empty.Description>
						Choose a report above to request a grounded study grouping suggestion.
					</Empty.Description>
				</Empty.Header>
			</Empty.Root>
		{:else if pending}
			<div class="flex flex-col gap-3" aria-label="Loading study grouping suggestion">
				<Skeleton class="h-5 w-2/3" />
				<Skeleton class="h-20 w-full" />
			</div>
		{:else if !proposal || !payload}
			<Empty.Root class="border-0 p-0">
				<Empty.Media variant="icon"><Info /></Empty.Media>
				<Empty.Header>
					<Empty.Title>No pending grouping suggestion</Empty.Title>
					<Empty.Description>
						Request a suggestion to compare this report with the project’s study groups.
					</Empty.Description>
				</Empty.Header>
			</Empty.Root>
		{:else}
			<div class="flex flex-wrap items-center gap-2">
				<Badge variant="secondary">{proposal.status}</Badge>
				<span class="text-xs text-muted-foreground">
					{proposal.provider} / {proposal.model} · prompt {proposal.prompt_version}
				</span>
			</div>
			<div class="rounded-md border p-4" data-testid="study-grouping-choice">
				<p class="text-sm font-medium">Suggested destination</p>
				{#if payload.choice.kind === 'existing_study'}
					<div class="mt-2 flex flex-wrap items-center gap-2">
						<Badge variant="secondary">Existing study</Badge>
						<span class="font-medium">{studyLabel(payload.choice.study_id)}</span>
						<span class="text-sm text-muted-foreground">
							study {payload.choice.study_id} · expected revision {payload.choice
								.expected_revision}
						</span>
					</div>
				{:else}
					<div class="mt-2 flex flex-wrap items-center gap-2">
						<Badge variant="secondary">New study</Badge>
						<span class="font-medium">{payload.choice.title}</span>
					</div>
				{/if}
				<p class="mt-2 text-xs text-muted-foreground">
					Accepting applies this proposed membership at the recorded revisions. The change
					can be reversed from the study membership controls.
				</p>
				<div class="mt-3 grid gap-2 text-sm sm:grid-cols-2">
					<div class="rounded-md bg-muted/40 p-2">
						<span class="font-medium">Current membership</span>
						<p class="text-xs text-muted-foreground">
							{membership?.study_id
								? `${studyLabel(membership.study_id)} · revision ${membership.study_revision}`
								: 'Unassigned'}
						</p>
					</div>
					<div class="rounded-md bg-muted/40 p-2">
						<span class="font-medium">Proposed previous membership</span>
						<p class="text-xs text-muted-foreground">
							{payload.expected_previous_study_id
								? `${studyLabel(payload.expected_previous_study_id)} · revision ${payload.expected_previous_study_revision ?? 'not provided'}`
								: 'No previous study'}
						</p>
					</div>
				</div>
			</div>
			<div class="rounded-md border p-4">
				<p class="text-sm font-medium">Rationale</p>
				<p class="mt-1 text-sm text-muted-foreground">{payload.rationale}</p>
			</div>
			<div class="rounded-md border p-4" data-testid="study-grouping-provenance">
				<p class="text-sm font-medium">Typed metadata provenance</p>
				{#if payload.provenance.length}
					<ul class="mt-2 flex flex-col gap-2 text-xs">
						{#each payload.provenance as evidence (evidence.kind + evidence.field + evidence.content_hash)}
							<li class="rounded-md bg-muted/40 p-2">
								<div class="flex flex-wrap gap-x-2 gap-y-1">
									<span class="font-medium">{evidenceSubject(evidence)}</span>
									<span class="text-muted-foreground"
										>· {fieldLabel(evidence.field)}</span
									>
								</div>
								<code class="mt-1 block break-all text-muted-foreground">
									content hash: {evidence.content_hash}
								</code>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="mt-1 text-xs text-muted-foreground">No provenance recorded.</p>
				{/if}
			</div>
			{#if payload.uncertainties.length}
				<Alert.Root role="status">
					<Alert.Title>Uncertainty noted</Alert.Title>
					<Alert.Description>
						<ul class="list-disc pl-5">
							{#each payload.uncertainties as uncertainty (uncertainty)}
								<li>{uncertainty}</li>
							{/each}
						</ul>
					</Alert.Description>
				</Alert.Root>
			{/if}
		{/if}
	</Card.Content>
	<Card.Footer class="flex flex-wrap justify-end gap-2">
		{#if reportId && !proposal}
			<Button variant="outline" onclick={onGenerate} disabled={action !== null}>
				{#if action === 'generate'}
					<Spinner data-icon="inline-start" />
				{:else}
					<Brain data-icon="inline-start" />
				{/if}
				Suggest study group
			</Button>
		{:else if proposal}
			<Button variant="outline" disabled={decisionPending} onclick={() => onDecide('reject')}>
				{#if action === 'reject'}
					<Spinner data-icon="inline-start" />
				{:else}
					<X data-icon="inline-start" />
				{/if}
				Reject
			</Button>
			<Button disabled={decisionPending} onclick={() => onDecide('accept')}>
				{#if action === 'accept'}
					<Spinner data-icon="inline-start" />
				{:else}
					<Check data-icon="inline-start" />
				{/if}
				Accept and apply
			</Button>
		{/if}
	</Card.Footer>
</Card.Root>
