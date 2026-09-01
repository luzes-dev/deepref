<script lang="ts">
	import type {
		AiProposalDecisionInput,
		AiProposalDto,
		AiStudyDesignClassificationProposalPayload,
		AiStudyDesignEvidenceDto
	} from '$lib/api/generated/models';
	import { StatePanel, Surface } from '$lib/components/layout';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Brain, Check, X } from '@lucide/svelte';

	let {
		proposal,
		pending,
		errorMessage,
		conflict,
		providerUnavailable,
		action,
		decisionPending,
		studyLabel,
		onDecide
	}: {
		proposal: AiProposalDto | undefined;
		pending: boolean;
		errorMessage: string;
		conflict: boolean;
		providerUnavailable: boolean;
		action: 'accept' | 'reject' | null;
		decisionPending: boolean;
		studyLabel: (studyId: string) => string;
		onDecide: (decision: AiProposalDecisionInput) => void;
	} = $props();

	function designLabel(
		design: NonNullable<AiStudyDesignClassificationProposalPayload['suggested_design']>
	): string {
		const labels = {
			rct: 'Randomized controlled trial',
			non_randomized_intervention: 'Non-randomized intervention',
			cohort: 'Cohort',
			case_control: 'Case-control',
			cross_sectional: 'Cross-sectional',
			diagnostic_accuracy: 'Diagnostic accuracy',
			prediction_model: 'Prediction model',
			qualitative: 'Qualitative',
			systematic_review: 'Systematic review',
			case_series: 'Case series'
		} satisfies Record<typeof design, string>;
		return labels[design];
	}

	function evidenceSubject(evidence: AiStudyDesignEvidenceDto): string {
		switch (evidence.kind) {
			case 'study_metadata':
				return `Study ${studyLabel(evidence.study_id)} · ${evidence.study_id}`;
			case 'report_metadata':
				return `Report ${evidence.report_id}`;
		}
	}

	function evidenceFieldLabel(evidence: AiStudyDesignEvidenceDto): string {
		if (evidence.kind === 'study_metadata') return 'Title';
		switch (evidence.field) {
			case 'title':
				return 'Title';
			case 'abstract':
				return 'Abstract';
			case 'publication_year':
				return 'Publication year';
		}
	}

	function evidenceKey(evidence: AiStudyDesignEvidenceDto): string {
		const sourceId =
			evidence.kind === 'study_metadata' ? evidence.study_id : evidence.report_id;
		return `${evidence.kind}:${sourceId}:${evidence.field}:${evidence.content_hash}`;
	}
</script>

<Card.Root data-testid="study-classification-assistance">
	<Card.Header class="gap-3">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<div class="flex items-center gap-2">
				<Brain aria-hidden="true" class="size-4" />
				<Card.Title>Study design classification assistance</Card.Title>
			</div>
			<Badge variant="outline">Proposal only</Badge>
		</div>
		<Card.Description>
			AI classifications are evidence-linked suggestions. A reviewer must approve or reject
			the proposal; accepting it records the reviewer decision and study history.
		</Card.Description>
	</Card.Header>
	<Card.Content class="flex flex-col gap-4">
		{#if errorMessage}
			<Alert.Root variant="destructive" role="alert">
				<Alert.Title>
					{conflict
						? 'Study classification changed elsewhere'
						: providerUnavailable
							? 'AI provider unavailable'
							: 'Study classification suggestion unavailable'}
				</Alert.Title>
				<Alert.Description>{errorMessage}</Alert.Description>
			</Alert.Root>
		{/if}
		{#if pending && !proposal}
			<div class="flex flex-col gap-3" aria-label="Loading study classification suggestion">
				<Skeleton class="h-5 w-2/3" />
				<Skeleton class="h-20 w-full" />
			</div>
		{:else if !proposal || proposal.payload.kind !== 'classification'}
			<StatePanel
				state="empty"
				title="No pending classification suggestion"
				description="The assistant has not created a study-design proposal for this study."
			/>
		{:else}
			{@const payload = proposal.payload}
			<div class="flex flex-wrap items-center gap-2">
				<Badge variant="secondary">{proposal.status}</Badge>
				<span class="text-xs text-muted-foreground">
					{proposal.provider} / {proposal.model} · prompt {proposal.prompt_version}
				</span>
			</div>
			<div data-testid="study-classification-suggestion">
				<Surface
					as="section"
					tone="inset"
					class="p-4"
					label="Suggested closed study design"
				>
					<p class="text-sm font-medium">Suggested closed study design</p>
					{#if payload.suggested_design}
						<div class="mt-2 flex flex-wrap items-center gap-2">
							<Badge variant="secondary"
								>{designLabel(payload.suggested_design)}</Badge
							>
							<code class="text-xs">{payload.suggested_design}</code>
						</div>
					{:else}
						<Badge class="mt-2" variant="outline">Abstention</Badge>
						<p class="mt-2 text-sm text-muted-foreground">
							The evidence does not support a closed study-design label.
						</p>
					{/if}
				</Surface>
			</div>
			<div>
				<Surface as="section" tone="inset" class="p-4" label="Classification rationale">
					<p class="text-sm font-medium">Rationale</p>
					<p class="mt-1 text-sm text-muted-foreground">{payload.rationale}</p>
				</Surface>
			</div>
			<div data-testid="study-classification-provenance">
				<Surface as="section" tone="inset" class="p-4" label="Evidence identity">
					<p class="text-sm font-medium">Evidence identity</p>
					<p class="mt-1 text-xs break-all text-muted-foreground">
						Prompt hash: {proposal.prompt_hash}
					</p>
					{#if payload.evidence.length}
						<ul class="mt-2 flex flex-col gap-2 text-xs">
							{#each payload.evidence as evidence (evidenceKey(evidence))}
								<li class="rounded-md bg-muted/40 p-2">
									<div class="flex flex-wrap gap-x-2 gap-y-1">
										<span class="font-medium">{evidenceSubject(evidence)}</span>
										<span class="text-muted-foreground">
											· {evidenceFieldLabel(evidence)} ({evidence.field})
										</span>
									</div>
									<code class="mt-1 block break-all text-muted-foreground">
										content hash: {evidence.content_hash}
									</code>
								</li>
							{/each}
						</ul>
					{:else}
						<p class="mt-1 text-xs text-muted-foreground">
							No evidence identity recorded.
						</p>
					{/if}
				</Surface>
			</div>
			{#if payload.uncertainties.length}
				<Alert.Root role="status" data-testid="study-classification-uncertainties">
					<Alert.Title>Uncertainties</Alert.Title>
					<Alert.Description>
						<ul class="list-disc pl-5">
							{#each payload.uncertainties as uncertainty (uncertainty)}
								<li>{uncertainty}</li>
							{/each}
						</ul>
					</Alert.Description>
				</Alert.Root>
			{:else}
				<p
					class="text-xs text-muted-foreground"
					data-testid="study-classification-uncertainties"
				>
					Uncertainties: none reported.
				</p>
			{/if}
			<div class="flex flex-wrap justify-end gap-2 border-t pt-4">
				<Button
					variant="outline"
					disabled={decisionPending}
					onclick={() => onDecide('reject')}
					data-testid="study-classification-reject"
				>
					{#if action === 'reject'}
						<Spinner data-icon="inline-start" />
					{:else}
						<X data-icon="inline-start" />
					{/if}
					Reject classification
				</Button>
				<Button
					disabled={decisionPending || !payload.suggested_design}
					onclick={() => onDecide('accept')}
					data-testid="study-classification-accept"
				>
					{#if action === 'accept'}
						<Spinner data-icon="inline-start" />
					{:else}
						<Check data-icon="inline-start" />
					{/if}
					Accept and apply classification
				</Button>
			</div>
		{/if}
	</Card.Content>
</Card.Root>
