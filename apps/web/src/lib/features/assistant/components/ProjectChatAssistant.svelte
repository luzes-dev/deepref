<script lang="ts">
	import {
		budgetReachedMessage,
		confirmAssistantPlan,
		createAssistantConversation,
		deleteAssistantConversation,
		deriveConversationTitle,
		fetchAiBudget,
		formatUsd,
		getAssistantPlan,
		listAssistantConversations,
		listAssistantMessages,
		rejectAssistantPlan,
		reviewQueuePathForTool,
		type AiBudget,
		type AssistantConversation,
		type AssistantMessageRecord,
		type AssistantPlan
	} from '../chat-api';
	import {
		emptyAssistantTurn,
		recordToTurn,
		reduceAssistantTurn,
		type ChatTurn
	} from '../chat-turns';
	import AssistantPlanCard from './AssistantPlanCard.svelte';
	import ChatMarkdown from './ChatMarkdown.svelte';
	import {
		AssistantStreamError,
		streamAssistantChat,
		type AssistantChatStreamEvent
	} from '#lib/api/assistant-stream.js';
	import { createGetAiStatus } from '#lib/api/generated/ai/ai.js';
	import * as Alert from '@deepref/ui/alert';
	import { Root as AvatarRoot, Fallback as AvatarFallback } from '@deepref/ui/avatar';
	import * as Attachment from '@deepref/ui/attachment';
	import { Button } from '@deepref/ui/button';
	import * as InputGroup from '@deepref/ui/input-group';
	import * as Marker from '@deepref/ui/marker';
	import * as Message from '@deepref/ui/message';
	import * as Bubble from '@deepref/ui/bubble';
	import { ProposalCard, ToolCallCard } from '@deepref/ui';
	import { ScrollArea } from '@deepref/ui/scroll-area';
	import { Spinner } from '@deepref/ui/spinner';
	import * as Sheet from '@deepref/ui/sheet';
	import BotIcon from '@lucide/svelte/icons/bot';
	import FileTextIcon from '@lucide/svelte/icons/file-text';
	import PanelLeftIcon from '@lucide/svelte/icons/panel-left';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import SendHorizontalIcon from '@lucide/svelte/icons/send-horizontal';
	import SquareIcon from '@lucide/svelte/icons/square';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import { tick } from 'svelte';

	let { projectId }: { projectId: string } = $props();

	// Optimistic until the workspace says otherwise: a failed status lookup must not block chat.
	const aiStatusQuery = createGetAiStatus();
	let providerMissing = $state(false);
	let budgetReached = $state(false);
	let budget = $state<AiBudget | null>(null);
	const assistantUnavailable = $derived(
		providerMissing || aiStatusQuery.data?.data.assistant_available === false
	);
	const budgetExhausted = $derived(budgetReached || budget?.exhausted === true);

	async function refreshBudget(): Promise<void> {
		try {
			budget = await fetchAiBudget(projectId);
			budgetReached = budget.exhausted;
		} catch {
			// The budget label is informational; chat must not depend on it.
		}
	}

	let stopController: AbortController | null = null;

	let conversations = $state<AssistantConversation[]>([]);
	let activeConversationId = $state<string | null>(null);
	let turns = $state<ChatTurn[]>([]);
	let draft = $state('');
	const starterQuestions = [
		'Which included reports still need a full-text PDF?',
		'Summarise what the included studies found about clinician workload.',
		'Do any included reports look like duplicates of each other?'
	] as const;
	let conversationsLoading = $state(true);
	let conversationsError = $state<string | null>(null);
	let threadError = $state<string | null>(null);
	let streamError = $state<string | null>(null);
	/**
	 * Set when live observation disconnects (user pressed Disconnect, or the
	 * events stream dropped) while the worker keeps running. The persisted
	 * answer converges into the thread via convergeAfterDisconnect.
	 */
	let disconnectNotice = $state<string | null>(null);
	let streamingTurnId = $state<string | null>(null);
	let sidebarOpen = $state(false);
	let feedViewport: HTMLElement | null = $state(null);
	let composerRef: HTMLTextAreaElement | null = $state(null);

	const isStreaming = $derived(streamingTurnId !== null);
	const activeConversation = $derived(
		conversations.find((conversation) => conversation.id === activeConversationId) ?? null
	);
	const latestRunningTool = $derived.by(() => {
		if (streamingTurnId === null) return null;
		const turn = turns.find((candidate) => candidate.id === streamingTurnId);
		if (!turn || turn.kind !== 'assistant') return null;
		return turn.tools.find((toolCall) => toolCall.status === 'running') ?? null;
	});
	const markerMessage = $derived(
		latestRunningTool
			? `DeepRef Assistant is executing ${latestRunningTool.tool}…`
			: 'DeepRef Assistant is thinking…'
	);

	$effect(() => {
		void refreshConversations(true);
		void refreshBudget();
	});

	$effect(() => {
		void draft.length;
		if (!composerRef) return;
		composerRef.style.height = 'auto';
		composerRef.style.height = `${Math.min(composerRef.scrollHeight, 192)}px`;
	});

	$effect(() => {
		const streamingTurn = turns.find(
			(turn) => turn.kind === 'assistant' && turn.id === streamingTurnId
		);
		let contentLength = 0;
		if (streamingTurn && streamingTurn.kind === 'assistant') {
			contentLength = streamingTurn.content.length;
		}
		void contentLength;
		void turns.length;
		scrollFeedIntoView();
	});

	async function scrollFeedIntoView(): Promise<void> {
		await tick();
		if (!feedViewport) return;
		const distance =
			feedViewport.scrollHeight - feedViewport.scrollTop - feedViewport.clientHeight;
		if (distance < 200) feedViewport.scrollTo({ top: feedViewport.scrollHeight });
	}

	function formatTime(value: string | null): string {
		if (!value) return '';
		const parsed = new Date(value);
		if (Number.isNaN(parsed.getTime())) return '';
		return new Intl.DateTimeFormat(undefined, {
			hour: '2-digit',
			minute: '2-digit'
		}).format(parsed);
	}

	function formatDate(value: string): string {
		const parsed = new Date(value);
		if (Number.isNaN(parsed.getTime())) return '';
		return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' }).format(
			parsed
		);
	}

	function toolSummaryLabel(tool: string): string {
		if (tool === 'propose_screening_decision') {
			return 'A screening proposal was queued for human review; the report stays unchanged until a reviewer signs off.';
		}
		if (tool === 'propose_duplicate_merge') {
			return 'A duplicate merge proposal was queued for human review; records stay unchanged until a reviewer signs off.';
		}
		if (tool.startsWith('propose_')) {
			return 'A proposal was queued for human review; the underlying record stays unchanged until a reviewer signs off.';
		}
		return 'The assistant finished this step and grounded its answer in the tool output below.';
	}

	function summarizedOutput(output: unknown): string | Record<string, unknown> | undefined {
		if (output === null || output === undefined) return undefined;
		if (typeof output === 'string') return output.slice(0, 400);
		if (typeof output === 'object') return output as Record<string, unknown>;
		return String(output);
	}

	async function refreshConversations(selectFirst: boolean): Promise<void> {
		conversationsLoading = true;
		conversationsError = null;
		try {
			conversations = await listAssistantConversations(projectId);
			if (selectFirst && conversations.length > 0 && activeConversationId === null) {
				await openConversation(conversations[0].id);
			}
		} catch (error: unknown) {
			conversationsError =
				error instanceof Error ? error.message : 'Conversations could not be loaded.';
		} finally {
			conversationsLoading = false;
		}
	}

	async function openConversation(conversationId: string): Promise<void> {
		if (isStreaming) return;
		activeConversationId = conversationId;
		sidebarOpen = false;
		threadError = null;
		streamError = null;
		disconnectNotice = null;
		try {
			const records = await listAssistantMessages(projectId, conversationId);
			turns = records
				.map((record) => recordToTurn(record, projectId))
				.filter((turn): turn is ChatTurn => turn !== null);
			await loadPlans();
			await scrollFeedIntoView();
		} catch (error: unknown) {
			threadError = error instanceof Error ? error.message : 'Messages could not be loaded.';
		}
	}

	async function loadPlans(): Promise<void> {
		await Promise.all(
			turns.map(async (turn) => {
				if (turn.kind !== 'assistant' || !turn.planId || turn.plan) return;
				try {
					turn.plan = await getAssistantPlan(projectId, turn.planId);
				} catch {
					turn.planError = 'This plan could not be loaded.';
				}
			})
		);
		turns = [...turns];
	}

	function hasQueuedReview(plan: AssistantPlan | null): boolean {
		return (plan?.results ?? []).some((result) => result.status === 'queued');
	}

	async function refreshQueuedPlans(): Promise<void> {
		await Promise.all(
			turns.map(async (turn) => {
				if (turn.kind !== 'assistant' || !turn.plan || !hasQueuedReview(turn.plan)) return;
				try {
					turn.plan = await getAssistantPlan(projectId, turn.plan.id);
				} catch {
					// Keep the last known state; the next refresh tries again.
				}
			})
		);
		turns = [...turns];
	}

	// A plan whose AI review is still running is refreshed until the review
	// finishes, so its card shows the real outcome without a reload.
	$effect(() => {
		if (!turns.some((turn) => turn.kind === 'assistant' && hasQueuedReview(turn.plan))) {
			return;
		}
		const timer = setInterval(() => void refreshQueuedPlans(), 10_000);
		return () => clearInterval(timer);
	});

	async function resolvePlan(turn: ChatTurn, action: 'confirm' | 'cancel'): Promise<void> {
		if (turn.kind !== 'assistant' || !turn.plan || turn.planBusy) return;
		const plan = turn.plan;
		turn.planBusy = true;
		turn.planError = null;
		turns = [...turns];
		try {
			turn.plan =
				action === 'confirm'
					? await confirmAssistantPlan(projectId, plan.id)
					: await rejectAssistantPlan(projectId, plan.id);
		} catch (error: unknown) {
			turn.planError =
				budgetReachedMessage(error) ??
				(error instanceof Error ? error.message : 'The plan could not be updated.');
			// The plan may have been resolved elsewhere; show its real state.
			try {
				turn.plan = await getAssistantPlan(projectId, plan.id);
			} catch {
				// keep the last known state
			}
		} finally {
			turn.planBusy = false;
			turns = [...turns];
		}
	}

	function startNewChat(): void {
		if (isStreaming) return;
		activeConversationId = null;
		turns = [];
		threadError = null;
		streamError = null;
		disconnectNotice = null;
		sidebarOpen = false;
		void tick().then(() => composerRef?.focus());
	}

	async function removeConversation(conversationId: string): Promise<void> {
		if (isStreaming) return;
		if (
			typeof window !== 'undefined' &&
			!window.confirm('Delete this conversation and its history?')
		)
			return;
		try {
			await deleteAssistantConversation(projectId, conversationId);
			conversations = conversations.filter(
				(conversation) => conversation.id !== conversationId
			);
			if (activeConversationId === conversationId) {
				activeConversationId = null;
				turns = [];
			}
		} catch (error: unknown) {
			threadError =
				error instanceof Error ? error.message : 'The conversation could not be deleted.';
		}
	}

	function applyStreamEvent(turnId: string, event: AssistantChatStreamEvent): void {
		if (event.event === 'error') {
			if (event.code === 'ai_budget_exceeded') budgetReached = true;
			streamError = event.message;
			return;
		}
		turns = turns.map((turn) =>
			turn.kind === 'assistant' && turn.id === turnId
				? reduceAssistantTurn(turn, event, projectId)
				: turn
		);
	}

	/**
	 * The plan frame can arrive Null (the run emits the plan event before the
	 * completion transaction inserts the plan row). The settled run carries
	 * plan_id, so fetch and render the plan when the stream did not deliver it.
	 */
	async function attachRunPlanIfMissing(
		turnId: string,
		planId: string | null | undefined
	): Promise<void> {
		if (!planId) return;
		const turn = turns.find(
			(candidate) => candidate.kind === 'assistant' && candidate.id === turnId
		);
		if (!turn || turn.kind !== 'assistant' || turn.plan) return;
		turn.planId = planId;
		try {
			turn.plan = await getAssistantPlan(projectId, planId);
		} catch {
			turn.planError = 'This plan could not be loaded.';
		}
		turns = [...turns];
	}

	/**
	 * After live observation disconnects, the worker keeps running and
	 * persists its answer. Poll the conversation until the persisted answer
	 * lands, then adopt the server state (with its plan) so the thread
	 * converges without reopening. Bounded and purely observational: it
	 * stops when the user navigates, sends again, or the poll budget runs out.
	 */
	async function convergeAfterDisconnect(
		conversationId: string,
		partialLength: number
	): Promise<void> {
		const maxAttempts = 24;
		const delayMs = 5_000;
		for (let attempt = 0; attempt < maxAttempts; attempt++) {
			if (attempt > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
			if (activeConversationId !== conversationId || isStreaming) return;
			let records: AssistantMessageRecord[];
			try {
				records = await listAssistantMessages(projectId, conversationId);
			} catch {
				continue;
			}
			if (activeConversationId !== conversationId || isStreaming) return;
			const serverTurns = records
				.map((record) => recordToTurn(record, projectId))
				.filter((turn): turn is ChatTurn => turn !== null);
			const latest = serverTurns.length > 0 ? serverTurns[serverTurns.length - 1] : null;
			const settled =
				latest &&
				latest.kind === 'assistant' &&
				((latest.content.length > 0 && latest.content.length >= partialLength) ||
					latest.planId !== null);
			if (settled) {
				turns = serverTurns;
				await loadPlans();
				if (activeConversationId !== conversationId || isStreaming) return;
				disconnectNotice = null;
				return;
			}
		}
	}

	function stopStreaming(): void {
		stopController?.abort();
	}

	async function send(): Promise<void> {
		const message = draft.trim();
		if (!message || isStreaming || assistantUnavailable || budgetExhausted) return;

		streamError = null;
		disconnectNotice = null;
		draft = '';

		if (activeConversationId === null) {
			try {
				const conversation = await createAssistantConversation(
					projectId,
					deriveConversationTitle(message)
				);
				conversations = [
					conversation,
					...conversations.filter((c) => c.id !== conversation.id)
				];
				activeConversationId = conversation.id;
				turns = [];
			} catch (error: unknown) {
				streamError =
					error instanceof Error
						? error.message
						: 'The conversation could not be created.';
				return;
			}
		}

		const conversationId = activeConversationId;
		const userTurn: ChatTurn = {
			kind: 'user',
			id: crypto.randomUUID(),
			content: message,
			createdAt: null
		};
		const assistantTurn = emptyAssistantTurn(crypto.randomUUID());
		turns = [...turns, userTurn, assistantTurn];
		streamingTurnId = assistantTurn.id;

		const controller = new AbortController();
		stopController = controller;
		try {
			const run = await streamAssistantChat(
				projectId,
				{ conversation_id: conversationId, message },
				(event) => applyStreamEvent(assistantTurn.id, event),
				controller.signal
			);
			await attachRunPlanIfMissing(assistantTurn.id, run?.plan_id ?? null);
		} catch (error: unknown) {
			if (controller.signal.aborted) {
				// Disconnect stops live observation only: the worker keeps the
				// run and persists its answer, which converges below.
				const partial = turns.find(
					(turn) => turn.kind === 'assistant' && turn.id === assistantTurn.id
				);
				const partialLength =
					partial && partial.kind === 'assistant' ? partial.content.length : 0;
				disconnectNotice = 'The assistant keeps working. Its answer will appear here.';
				void convergeAfterDisconnect(conversationId, partialLength);
			} else if (error instanceof AssistantStreamError && error.status === 503) {
				providerMissing = true;
				turns = turns.filter((t) => t.id !== userTurn.id && t.id !== assistantTurn.id);
				draft = message;
			} else if (
				error instanceof AssistantStreamError &&
				error.code === 'ai_budget_exceeded'
			) {
				budgetReached = true;
				turns = turns.filter((t) => t.id !== userTurn.id && t.id !== assistantTurn.id);
				draft = message;
			} else {
				streamError =
					error instanceof Error
						? error.message
						: 'The assistant stream failed unexpectedly.';
			}
		} finally {
			stopController = null;
			streamingTurnId = null;
			void refreshConversations(false);
			void refreshBudget();
		}
	}

	function onComposerKeyDown(event: KeyboardEvent): void {
		if (event.key === 'Enter' && !event.shiftKey) {
			event.preventDefault();
			void send();
		}
	}
</script>

<div
	class="relative flex h-full min-h-0 w-full overflow-hidden bg-background"
	data-testid="assistant-page"
>
	<Sheet.Root bind:open={sidebarOpen}>
		<Sheet.Content
			side="left"
			class="w-80 max-w-[90vw] gap-0"
			onCloseAutoFocus={() =>
				document
					.querySelector<HTMLButtonElement>('[data-testid="assistant-sessions-toggle"]')
					?.focus()}
		>
			<Sheet.Header class="pr-12">
				<Sheet.Title>Conversations</Sheet.Title>
				<Sheet.Description>Return to a saved session.</Sheet.Description>
			</Sheet.Header>
			<div class="min-h-0 flex-1 overflow-y-auto p-2">
				{#if conversationsError}
					<div class="p-2">
						<Alert.Root variant="destructive" role="alert">
							<Alert.Title>Conversations unavailable</Alert.Title>
							<Alert.Description>{conversationsError}</Alert.Description>
							<Button
								variant="outline"
								size="sm"
								onclick={() => void refreshConversations(true)}>Retry</Button
							>
						</Alert.Root>
					</div>
				{:else if conversationsLoading}
					<div class="flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground">
						<Spinner /> Loading conversations
					</div>
				{:else if conversations.length === 0}
					<p class="px-3 py-2 text-sm text-muted-foreground">No conversations yet.</p>
				{:else}
					<ul class="flex flex-col gap-1">
						{#each conversations as conversation (conversation.id)}
							<li class="group relative">
								<button
									type="button"
									class="w-full rounded-lg px-3 py-2 pr-9 text-left transition-colors {activeConversationId ===
									conversation.id
										? 'bg-muted/50 text-foreground'
										: 'text-muted-foreground hover:bg-muted/25 hover:text-foreground'}"
									onclick={() => void openConversation(conversation.id)}
									disabled={isStreaming}
									data-testid="assistant-thread"
								>
									<span class="block truncate text-sm font-medium">
										{conversation.title}
									</span>
									<span class="mt-0.5 block text-xs text-muted-foreground">
										{formatDate(conversation.updated_at)}
									</span>
								</button>
								<button
									type="button"
									class="absolute top-1/2 right-2 -translate-y-1/2 rounded-md p-1.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:bg-muted hover:text-destructive focus-visible:opacity-100"
									aria-label="Delete conversation {conversation.title}"
									onclick={() => void removeConversation(conversation.id)}
								>
									<Trash2Icon class="size-4" aria-hidden="true" />
								</button>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		</Sheet.Content>
	</Sheet.Root>

	<section class="flex min-w-0 flex-1 flex-col">
		<header class="flex items-center gap-3 border-b border-border/70 px-4 py-3 sm:px-6">
			<Button
				variant="ghost"
				size="icon"
				class="shrink-0"
				aria-label="Toggle conversations"
				aria-expanded={sidebarOpen}
				data-testid="assistant-sessions-toggle"
				onclick={() => (sidebarOpen = !sidebarOpen)}
			>
				<PanelLeftIcon aria-hidden="true" />
			</Button>
			<div class="min-w-0 flex-1">
				<h1 class="text-base font-semibold text-foreground">
					<span class="block truncate"
						>{activeConversation?.title ?? 'New conversation'}</span
					>
				</h1>
				<p class="text-xs text-muted-foreground">
					Ask about this project's evidence. Changes always wait for your confirmation.
					{#if budget && !budget.exhausted}
						· {formatUsd(budget.remaining_usd)} of AI budget left this month
					{/if}
				</p>
			</div>
			<Button
				class="ml-auto shrink-0"
				variant="outline"
				size="sm"
				onclick={startNewChat}
				disabled={isStreaming || assistantUnavailable}
			>
				<PlusIcon data-icon="inline-start" /> New chat
			</Button>
		</header>

		<ScrollArea class="min-h-0 flex-1" bind:viewportRef={feedViewport}>
			<div class="mx-auto w-full max-w-3xl" data-testid="assistant-feed">
				{#if threadError}
					<div class="p-4">
						<Alert.Root variant="destructive" role="alert">
							<Alert.Title>Thread unavailable</Alert.Title>
							<Alert.Description>{threadError}</Alert.Description>
						</Alert.Root>
					</div>
				{/if}
				<Message.Group>
					{#each turns as turn (turn.id)}
						{#if turn.kind === 'user'}
							<Message.Root align="end">
								<Bubble.Root variant="default" size="compact" class="max-w-[85%]">
									<p
										class="text-sm leading-relaxed break-words whitespace-pre-wrap"
									>
										{turn.content}
									</p>
								</Bubble.Root>
								<AvatarRoot class="size-7 shrink-0 self-start">
									<AvatarFallback class="text-xs">U</AvatarFallback>
								</AvatarRoot>
							</Message.Root>
						{:else}
							<Message.Root align="start">
								<Message.Avatar>
									<AvatarRoot class="size-7 bg-primary/10 text-primary">
										<AvatarFallback class="bg-primary/10 text-primary">
											<BotIcon class="size-4" aria-hidden="true" />
										</AvatarFallback>
									</AvatarRoot>
								</Message.Avatar>
								<div class="flex max-w-[85%] min-w-0 flex-col gap-1.5">
									<Message.Header>
										<span class="font-medium text-foreground"
											>DeepRef Assistant</span
										>
										{#if turn.createdAt}
											<span>{formatTime(turn.createdAt)}</span>
										{/if}
									</Message.Header>
									<Bubble.Group>
										{#if turn.content}
											<Bubble.Root variant="muted" size="compact">
												<ChatMarkdown source={turn.content} />
											</Bubble.Root>
										{/if}
										{#if turn.tools.length > 0}
											<details class="border-l pl-3">
												<summary
													class="cursor-pointer py-2 text-xs font-medium text-muted-foreground"
												>
													Evidence &amp; tools · {turn.tools.length}
													{turn.tools.length === 1 ? 'step' : 'steps'}
												</summary>
												{#each turn.tools as toolCall (toolCall.toolCallId)}
													<ToolCallCard
														toolName={toolCall.tool}
														arguments={toolCall.args ?? {}}
														status={toolCall.status}
														outputSummary={toolCall.status ===
														'completed'
															? summarizedOutput(toolCall.output)
															: undefined}
													/>
												{/each}
											</details>
										{/if}
										{#each turn.proposals as proposal (proposal.reviewRunId)}
											<ProposalCard
												kind={proposal.tool}
												summary={toolSummaryLabel(proposal.tool)}
												targetId={proposal.reviewRunId}
												targetLabel="Review run"
												reviewHref={reviewQueuePathForTool(
													proposal.tool,
													projectId
												) ?? '#'}
											/>
										{/each}
										{#if turn.plan}
											<AssistantPlanCard
												{projectId}
												plan={turn.plan}
												busy={turn.planBusy}
												error={turn.planError}
												onconfirm={() => void resolvePlan(turn, 'confirm')}
												oncancel={() => void resolvePlan(turn, 'cancel')}
											/>
										{:else if turn.planError}
											<Alert.Root variant="destructive" role="alert">
												<Alert.Description
													>{turn.planError}</Alert.Description
												>
											</Alert.Root>
										{/if}
										{#if turn.id === streamingTurnId && (latestRunningTool || !turn.content)}
											<Marker.Root data-testid="assistant-streaming">
												<Marker.Content>{markerMessage}</Marker.Content>
											</Marker.Root>
										{/if}
									</Bubble.Group>
									{#if turn.citations.length > 0}
										<div class="flex flex-wrap gap-1.5">
											{#each turn.citations as citation (citation.key)}
												<Attachment.Root
													href={citation.href ?? undefined}
													title={citation.tooltip}
													data-testid="assistant-citation"
												>
													<Attachment.Preview>
														<FileTextIcon
															class="size-3"
															aria-hidden="true"
														/>
													</Attachment.Preview>
													<Attachment.Name
														>{citation.label}</Attachment.Name
													>
												</Attachment.Root>
											{/each}
										</div>
									{/if}
								</div>
							</Message.Root>
						{/if}
					{/each}
					{#if assistantUnavailable}
						<div
							class="mx-auto flex w-full max-w-3xl flex-col items-start gap-4 px-6 py-10"
							data-testid="assistant-unavailable"
						>
							<BotIcon class="size-6 text-muted-foreground" aria-hidden="true" />
							<div class="flex flex-col gap-1">
								<p class="font-medium text-foreground">
									The assistant isn't set up for this workspace
								</p>
								<p class="max-w-xl text-sm text-muted-foreground">
									It needs an AI provider. Providers are configured by your
									workspace administrator through the server configuration; once
									one is enabled, you can ask about this project's evidence here.
								</p>
							</div>
						</div>
					{:else if turns.length === 0 && !threadError}
						<div
							class="mx-auto flex w-full max-w-3xl flex-col items-start gap-4 px-6 py-10"
							data-testid="assistant-empty-state"
						>
							<BotIcon class="size-6 text-muted-foreground" aria-hidden="true" />
							<div class="flex flex-col gap-1">
								<p class="font-medium text-foreground">
									Ask this project's evidence anything
								</p>
								<p class="max-w-xl text-sm text-muted-foreground">
									It reads the protocol, reports and documents, and can queue
									review proposals for you to approve. It never changes records
									directly.
								</p>
							</div>
							<div
								class="flex flex-col items-start gap-1"
								aria-label="Suggested questions"
							>
								{#each starterQuestions as question (question)}
									<button
										type="button"
										class="rounded-md px-2 py-1.5 text-left text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
										disabled={isStreaming}
										onclick={() => {
											draft = question;
											void send();
										}}>{question}</button
									>
								{/each}
							</div>
						</div>
					{/if}
				</Message.Group>
			</div>
		</ScrollArea>

		<div class="border-t border-border/70 px-4 py-3 sm:px-6">
			<div class="mx-auto w-full max-w-3xl">
				{#if budgetExhausted}
					<Alert.Root
						variant="warning"
						class="mb-3"
						data-testid="assistant-budget-reached"
					>
						<Alert.Title>AI budget for this month reached</Alert.Title>
						<Alert.Description>
							The assistant is paused until next month. Ask a project administrator to
							raise the monthly AI budget if you need it sooner.
						</Alert.Description>
					</Alert.Root>
				{:else if streamError}
					<Alert.Root
						variant="destructive"
						role="alert"
						class="mb-3"
						data-testid="assistant-stream-error"
					>
						<Alert.Title>The assistant turn failed</Alert.Title>
						<Alert.Description>{streamError}</Alert.Description>
					</Alert.Root>
				{:else if disconnectNotice}
					<Alert.Root
						variant="info"
						role="status"
						class="mb-3"
						data-testid="assistant-disconnected"
					>
						<Alert.Title>Disconnected from live updates</Alert.Title>
						<Alert.Description>{disconnectNotice}</Alert.Description>
					</Alert.Root>
				{/if}
				<InputGroup.Root
					class="items-end gap-2 rounded-xl border border-border/80 bg-card p-2"
				>
					<InputGroup.Textarea
						bind:ref={composerRef}
						bind:value={draft}
						rows={1}
						placeholder="Ask about this project's evidence…"
						aria-label="Message the assistant"
						onkeydown={onComposerKeyDown}
						disabled={isStreaming || assistantUnavailable || budgetExhausted}
						data-testid="assistant-thread-input"
					/>
					{#if isStreaming}
						<Button
							size="icon"
							variant="outline"
							class="size-9 shrink-0"
							aria-label="Disconnect — the assistant keeps working"
							title="Disconnect — the assistant keeps working"
							onclick={stopStreaming}
							data-testid="assistant-stop"
						>
							<SquareIcon class="size-3.5 fill-current" aria-hidden="true" />
						</Button>
					{:else}
						<Button
							size="icon"
							class="size-9 shrink-0"
							aria-label="Send message"
							onclick={() => void send()}
							disabled={isStreaming ||
								assistantUnavailable ||
								budgetExhausted ||
								draft.trim().length === 0}
							data-testid="assistant-send"
						>
							<SendHorizontalIcon aria-hidden="true" />
						</Button>
					{/if}
				</InputGroup.Root>
				<p class="mt-2 text-center text-2xs text-muted-foreground">
					{#if isStreaming}
						Disconnect stops live updates — the assistant keeps working.
					{:else}
						Enter to send · Shift + Enter for a new line
					{/if}
				</p>
			</div>
		</div>
	</section>
</div>
