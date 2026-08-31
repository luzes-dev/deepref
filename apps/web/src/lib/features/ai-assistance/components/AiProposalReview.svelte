<script lang="ts">
	import {
		createDecideAiProposal,
		createGenerateDuplicateSuggestion,
		createGenerateScreeningSuggestion,
		createListAiProposals
	} from '$lib/api/generated/ai/ai';
	import type {
		AiDuplicateSignalDto,
		AiIdentityProvenanceDto,
		AiProposalDecisionInput,
		AiProposalDto,
		AiScreeningEvidenceDto,
		AiScreeningStageInput
	} from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Brain, Check, FileSearch, Info, X } from '@lucide/svelte';
	import { ReviewRunObserver } from '../review-run-observer.svelte';

	type ReviewStage = 'title_abstract' | 'full_text' | 'dedupe';
	type DocumentBlockEvidence = Extract<AiScreeningEvidenceDto, { kind: 'document_block' }>;

	let {
		projectId,
		reportId = null,
		recordId = null,
		candidateReportId = null,
		stage,
		protocolVersionId = null,
		expectedRevision = null,
		onEvidenceSelect = () => undefined
	}: {
		projectId: string;
		reportId?: string | null;
		recordId?: string | null;
		candidateReportId?: string | null;
		stage: ReviewStage;
		protocolVersionId?: string | null;
		expectedRevision?: number | null;
		onEvidenceSelect?: (evidence: DocumentBlockEvidence) => void;
	} = $props();

	const taskKind = $derived(
		stage === 'title_abstract'
			? 'title_abstract_screening'
			: stage === 'full_text'
				? 'full_text_screening'
				: 'duplicate_candidate_detection'
	);
	const proposalsQuery = createListAiProposals(
		() => projectId,
		() => ({
			status: 'pending',
			task_kind: taskKind,
			limit: 1,
			...(stage === 'dedupe'
				? {
						...(recordId ? { target_record_id: recordId } : {}),
						...(candidateReportId ? { candidate_report_id: candidateReportId } : {})
					}
				: reportId
					? { target_report_id: reportId }
					: {})
		})
	);
	const generateScreening = createGenerateScreeningSuggestion();
	const generateDuplicate = createGenerateDuplicateSuggestion();
	const decideProposal = createDecideAiProposal();
	const reviewRun = new ReviewRunObserver(
		() => projectId,
		async () => {
			await proposalsQuery.refetch();
		}
	);

	let pendingProposalId = $state<string | null>(null);
	let localError = $state('');

	const proposals = $derived(proposalsQuery.data?.data.items ?? []);
	const activeProposal = $derived(proposals[0] ?? null);
	const errorMessage = $derived(
		localError ||
			reviewRun.error ||
			proposalsQuery.error?.message ||
			generateScreening.error?.message ||
			generateDuplicate.error?.message ||
			decideProposal.error?.message ||
			''
	);

	function criterionRows(proposal: AiProposalDto) {
		switch (proposal.payload.kind) {
			case 'screening':
				return proposal.payload.criteria;
			case 'duplicate':
			case 'study_grouping':
			case 'appraisal_prefill':
			case 'data_extraction':
				return [];
			default: {
				const exhaustive: never = proposal.payload;
				return exhaustive;
			}
		}
	}

	function suggestedDecision(proposal: AiProposalDto): string {
		switch (proposal.payload.kind) {
			case 'screening':
				return proposal.payload.suggested_decision.kind;
			case 'duplicate':
				return proposal.payload.decision;
			case 'study_grouping':
			case 'appraisal_prefill':
			case 'data_extraction':
				return 'unavailable';
			default: {
				const exhaustive: never = proposal.payload;
				return exhaustive;
			}
		}
	}

	function canApprove(proposal: AiProposalDto): boolean {
		switch (proposal.payload.kind) {
			case 'screening':
				return proposal.payload.suggested_decision.kind !== 'insufficient_evidence';
			case 'duplicate':
				return proposal.payload.decision === 'match';
			case 'study_grouping':
			case 'appraisal_prefill':
			case 'data_extraction':
				return false;
			default: {
				const exhaustive: never = proposal.payload;
				return exhaustive;
			}
		}
	}

	function dedupeRationales(proposal: AiProposalDto) {
		return proposal.payload.kind === 'duplicate' ? proposal.payload.rationale : [];
	}

	function dedupeSignals(proposal: AiProposalDto) {
		return proposal.payload.kind === 'duplicate' ? proposal.payload.signals : [];
	}

	function dedupeProvenance(proposal: AiProposalDto): AiIdentityProvenanceDto[] {
		return proposal.payload.kind === 'duplicate' ? proposal.payload.provenance : [];
	}

	function dedupeSignalLabel(signal: AiDuplicateSignalDto): string {
		switch (signal.kind) {
			case 'title_similarity':
				return `Title similarity ${(signal.similarity * 100).toFixed(1)}%`;
			case 'publication_year':
				return `Publication year ${signal.source_year} → ${signal.candidate_year}`;
			case 'first_author':
				return `First author ${signal.source_author} → ${signal.candidate_author} · ${(signal.similarity * 100).toFixed(1)}%`;
			case 'durable_identifier':
				return `${signal.scheme} ${signal.source_value} → ${signal.candidate_value}`;
			default: {
				const exhaustive: never = signal;
				return exhaustive;
			}
		}
	}

	function provenanceLabel(provenance: AiIdentityProvenanceDto): string {
		const entity =
			provenance.entity_type === 'record'
				? 'Source record'
				: provenance.entity_type === 'report'
					? 'Candidate report'
					: provenance.entity_type;
		return `${entity} ${provenance.entity_id.slice(0, 8)} · ${provenance.field} · hash ${provenance.content_hash.slice(0, 12)}…`;
	}

	function uncertainties(proposal: AiProposalDto): string[] {
		switch (proposal.payload.kind) {
			case 'screening':
			case 'duplicate':
			case 'study_grouping':
				return proposal.payload.uncertainties;
			case 'appraisal_prefill':
			case 'data_extraction':
				return [];
			default: {
				const exhaustive: never = proposal.payload;
				return exhaustive;
			}
		}
	}

	function candidateReportLabel(proposal: AiProposalDto): string {
		return proposal.payload.kind === 'duplicate'
			? proposal.payload.candidate.candidate_report_id
			: 'unknown';
	}

	function evidenceLabel(evidence: DocumentBlockEvidence): string {
		const section = evidence.section_path.length
			? ` · ${evidence.section_path.join(' / ')}`
			: '';
		return `Page ${evidence.page}${section}`;
	}

	function metadataLabel(field: 'title' | 'abstract'): string {
		return field === 'title' ? 'Title' : 'Abstract';
	}

	function isConflict(error: unknown): boolean {
		return error instanceof ApiError && error.status === 409;
	}

	async function generate() {
		if (generateScreening.isPending || generateDuplicate.isPending || reviewRun.isActive)
			return;
		localError = '';
		try {
			if (stage === 'dedupe') {
				if (!recordId || !candidateReportId) return;
				const response = await generateDuplicate.mutateAsync({
					projectId,
					recordId,
					data: { candidate_report_id: candidateReportId }
				});
				await reviewRun.observe(response.data);
			} else {
				if (!reportId) return;
				const request: {
					stage: AiScreeningStageInput;
					protocol_version_id?: string;
					expected_revision?: number;
				} = {
					stage,
					...(protocolVersionId ? { protocol_version_id: protocolVersionId } : {}),
					...(expectedRevision !== null ? { expected_revision: expectedRevision } : {})
				};
				const response = await generateScreening.mutateAsync({
					projectId,
					reportId,
					data: request
				});
				await reviewRun.observe(response.data);
			}
		} catch (error) {
			localError = error instanceof Error ? error.message : 'AI suggestion failed.';
		}
	}

	async function decide(proposal: AiProposalDto, decision: AiProposalDecisionInput) {
		if (pendingProposalId) return;
		pendingProposalId = proposal.id;
		localError = '';
		try {
			await decideProposal.mutateAsync({
				projectId,
				proposalId: proposal.id,
				data: {
					decision,
					reason: `Human reviewer ${decision}d AI ${stage} suggestion.`
				}
			});
			await proposalsQuery.refetch();
		} catch (error) {
			localError = isConflict(error)
				? 'This proposal or screening revision changed elsewhere. The current queue was refreshed.'
				: error instanceof Error
					? error.message
					: 'The proposal decision failed.';
			await proposalsQuery.refetch();
		} finally {
			pendingProposalId = null;
		}
	}
</script>

<Card.Root data-testid="ai-proposal-review">
	<Card.Header class="gap-3">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<div class="flex items-center gap-2">
				<Brain aria-hidden="true" class="size-4" />
				<Card.Title>AI assistance</Card.Title>
			</div>
			<Badge variant="outline">Proposal only</Badge>
		</div>
		<Card.Description>
			AI suggestions are grounded in the recorded protocol and evidence. A reviewer must
			approve any consequential action.
		</Card.Description>
	</Card.Header>
	<Card.Content class="flex flex-col gap-4">
		{#if errorMessage}
			<Alert.Root variant="destructive" role="alert">
				<Alert.Title>AI assistance needs attention</Alert.Title>
				<Alert.Description>{errorMessage}</Alert.Description>
			</Alert.Root>
		{:else if proposalsQuery.isPending}
			<div class="flex flex-col gap-3" aria-label="Loading AI assistance">
				<Skeleton class="h-5 w-2/3" />
				<Skeleton class="h-16 w-full" />
			</div>
		{:else if !activeProposal}
			<Empty.Root class="border-0 p-0">
				<Empty.Media variant="icon"><Info /></Empty.Media>
				<Empty.Header>
					<Empty.Title>No pending suggestion</Empty.Title>
					<Empty.Description>
						{stage === 'dedupe'
							? 'Generate a candidate assessment, then review it here.'
							: 'Request a grounded suggestion for this report when you are ready.'}
					</Empty.Description>
				</Empty.Header>
			</Empty.Root>
		{:else}
			{@const proposal = activeProposal}
			<div class="flex flex-wrap items-center gap-2">
				<Badge variant={proposal.status === 'pending' ? 'secondary' : 'outline'}>
					{proposal.status}
				</Badge>
				<Badge variant="outline">{suggestedDecision(proposal)}</Badge>
				<span class="text-xs text-muted-foreground">
					{proposal.provider} / {proposal.model} · prompt {proposal.prompt_version}
				</span>
			</div>

			{#if stage !== 'dedupe'}
				<div class="flex flex-col gap-3" aria-label="Criterion judgments">
					{#each criterionRows(proposal) as criterion (criterion.criterion_id)}
						<div class="rounded-md border p-3">
							<div class="flex flex-wrap items-center justify-between gap-2">
								<span class="font-medium">{criterion.criterion_label}</span>
								<Badge variant="secondary">{criterion.judgment}</Badge>
							</div>
							<p class="mt-2 text-sm text-muted-foreground">{criterion.rationale}</p>
							{#if criterion.evidence.length}
								<div class="mt-2 flex flex-wrap gap-2">
									{#each criterion.evidence as evidence (evidence.kind === 'document_block' ? evidence.document_block_id + evidence.page : evidence.report_id + evidence.field)}
										{#if evidence.kind === 'document_block'}
											<Button
												variant="outline"
												size="sm"
												onclick={() => onEvidenceSelect(evidence)}
											>
												<FileSearch data-icon="inline-start" />
												{evidenceLabel(evidence)}
											</Button>
										{:else}
											<div class="rounded-md bg-muted px-2 py-1 text-xs">
												Report metadata · {metadataLabel(evidence.field)} · hash
												<span class="font-mono"
													>{evidence.content_hash.slice(0, 12)}…</span
												>
											</div>
										{/if}
									{/each}
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{:else}
				<div class="rounded-md border p-3 text-sm">
					<p class="font-medium">Candidate pair</p>
					<p class="mt-1 text-muted-foreground">
						Record {proposal.target_record_id ?? 'unknown'} · report {candidateReportLabel(
							proposal
						)}
					</p>
				</div>
				{#if dedupeRationales(proposal).length}
					<div class="rounded-md border p-3 text-sm">
						<p class="font-medium">Rationale</p>
						<ul class="mt-1 list-disc pl-5 text-muted-foreground">
							{#each dedupeRationales(proposal) as rationale (rationale.code)}
								<li>{rationale.code}: {rationale.explanation}</li>
							{/each}
						</ul>
					</div>
				{/if}
				{#if dedupeSignals(proposal).length}
					<div class="rounded-md border p-3 text-sm">
						<p class="font-medium">Signals</p>
						<ul class="mt-1 list-disc pl-5 text-muted-foreground">
							{#each dedupeSignals(proposal) as signal (signal.kind)}
								<li>
									{dedupeSignalLabel(signal)} · {signal.supports_match
										? 'supports match'
										: 'does not support match'}
								</li>
							{/each}
						</ul>
					</div>
				{/if}
				{#if dedupeProvenance(proposal).length}
					<div class="rounded-md border p-3 text-sm" data-testid="ai-dedupe-provenance">
						<p class="font-medium">Evidence provenance</p>
						<ul class="mt-1 flex flex-col gap-1 text-muted-foreground">
							{#each dedupeProvenance(proposal) as evidence (evidence.entity_type + evidence.entity_id + evidence.field)}
								<li>{provenanceLabel(evidence)}</li>
							{/each}
						</ul>
					</div>
				{/if}
			{/if}

			{#if uncertainties(proposal).length}
				<div
					class="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-sm"
					role="status"
				>
					<p class="font-medium">Uncertainty / abstention</p>
					<ul class="mt-1 list-disc pl-5">
						{#each uncertainties(proposal) as uncertainty (uncertainty)}
							<li>{uncertainty}</li>
						{/each}
					</ul>
				</div>
			{/if}
		{/if}
	</Card.Content>
	<Card.Footer class="flex flex-wrap justify-end gap-2">
		{#if !activeProposal && ((stage === 'dedupe' && recordId && candidateReportId) || stage !== 'dedupe')}
			<Button
				variant="outline"
				onclick={() => void generate()}
				disabled={generateScreening.isPending ||
					generateDuplicate.isPending ||
					reviewRun.isActive}
			>
				{#if generateScreening.isPending || generateDuplicate.isPending || reviewRun.isActive}<Spinner
						data-icon="inline-start"
					/>{:else}<Brain data-icon="inline-start" />{/if}
				Request suggestion
			</Button>
		{/if}
		{#if activeProposal}
			{@const proposal = activeProposal}
			<Button
				variant="outline"
				disabled={pendingProposalId !== null}
				onclick={() => void decide(proposal, 'reject')}
			>
				{#if pendingProposalId === proposal.id}<Spinner data-icon="inline-start" />{:else}<X
						data-icon="inline-start"
					/>{/if}
				Reject
			</Button>
			<Button
				disabled={pendingProposalId !== null || !canApprove(proposal)}
				onclick={() => void decide(proposal, 'accept')}
			>
				{#if pendingProposalId === proposal.id}<Spinner
						data-icon="inline-start"
					/>{:else}<Check data-icon="inline-start" />{/if}
				Approve and apply
			</Button>
		{/if}
	</Card.Footer>
</Card.Root>
