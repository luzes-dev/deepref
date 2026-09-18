<script lang="ts">
	import {
		createAssistantConversation,
		deleteAssistantConversation,
		deriveConversationTitle,
		listAssistantConversations,
		listAssistantMessages,
		reviewQueuePathForTool,
		type AssistantConversation,
		type AssistantMessageRecord
	} from '../chat-api';
	import { streamAssistantChat, type AssistantChatStreamEvent } from '$lib/api/assistant-stream';
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
	import BotIcon from '@lucide/svelte/icons/bot';
	import FileTextIcon from '@lucide/svelte/icons/file-text';
	import PanelLeftIcon from '@lucide/svelte/icons/panel-left';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import SendHorizontalIcon from '@lucide/svelte/icons/send-horizontal';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import { tick } from 'svelte';

	let { projectId }: { projectId: string } = $props();

	type ToolCallView = {
		toolCallId: string;
		tool: string;
		args: Record<string, unknown> | null;
		status: 'running' | 'completed' | 'failed';
		output: unknown;
	};

	type ProposalView = {
		tool: string;
		reviewRunId: string;
	};

	type ChatTurn =
		| { kind: 'user'; id: string; content: string; createdAt: string | null }
		| {
				kind: 'assistant';
				id: string;
				content: string;
				createdAt: string | null;
				tools: ToolCallView[];
				proposals: ProposalView[];
				citations: string[];
				tokens: { input: number; output: number } | null;
		  };

	let conversations = $state<AssistantConversation[]>([]);
	let activeConversationId = $state<string | null>(null);
	let turns = $state<ChatTurn[]>([]);
	let draft = $state('');
	let conversationsLoading = $state(true);
	let conversationsError = $state<string | null>(null);
	let threadError = $state<string | null>(null);
	let streamError = $state<string | null>(null);
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

	function shortId(value: string): string {
		return value.slice(0, 8);
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

	function harvestCitations(tool: string, output: unknown): string[] {
		if (!output || typeof output !== 'object') return [];
		const label =
			tool === 'get_project_protocol'
				? 'Protocol'
				: tool.startsWith('read_document') || tool.startsWith('search_document')
					? 'Block'
					: tool.startsWith('search_project_reports') || tool === 'get_report'
						? 'Report'
						: 'Record';
		const collect = (candidate: unknown): string[] => {
			if (!candidate || typeof candidate !== 'object') return [];
			if (Array.isArray(candidate)) {
				return candidate.flatMap((item) => collect(item));
			}
			const record = candidate as Record<string, unknown>;
			const id = typeof record['id'] === 'string' ? record['id'] : null;
			const nested = Object.values(record).flatMap((value) =>
				typeof value === 'object' && value !== null ? collect(value) : []
			);
			return id ? [id, ...nested] : nested;
		};
		const ids = [...new Set(collect(output))];
		if (ids.length === 0) return [];
		return ids.slice(0, 4).map((id) => `${label} ${shortId(id)}`);
	}

	function summarizedOutput(output: unknown): string | Record<string, unknown> | undefined {
		if (output === null || output === undefined) return undefined;
		if (typeof output === 'string') return output.slice(0, 400);
		if (typeof output === 'object') return output as Record<string, unknown>;
		return String(output);
	}

	function emptyAssistantTurn(): ChatTurn {
		return {
			kind: 'assistant',
			id: crypto.randomUUID(),
			content: '',
			createdAt: null,
			tools: [],
			proposals: [],
			citations: [],
			tokens: null
		};
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
		activeConversationId = conversationId;
		sidebarOpen = false;
		threadError = null;
		streamError = null;
		try {
			const records = await listAssistantMessages(projectId, conversationId);
			turns = records.map(recordToTurn).filter((turn): turn is ChatTurn => turn !== null);
			await scrollFeedIntoView();
		} catch (error: unknown) {
			threadError = error instanceof Error ? error.message : 'Messages could not be loaded.';
		}
	}

	function recordToTurn(record: AssistantMessageRecord): ChatTurn | null {
		if (record.role === 'user') {
			return {
				kind: 'user',
				id: record.id,
				content: record.content,
				createdAt: record.created_at
			};
		}
		if (record.role !== 'assistant') return null;

		const tools: ToolCallView[] = (record.tool_calls ?? []).map((call) => {
			const result = (record.tool_results ?? []).find(
				(candidate) => candidate.tool_call_id === call.id
			);
			return {
				toolCallId: call.id,
				tool: call.tool,
				args: call.args,
				status: result ? 'completed' : 'failed',
				output: result?.output ?? null
			};
		});
		const proposals: ProposalView[] = (record.tool_results ?? [])
			.filter((result) => typeof result.proposal_review_run_id === 'string')
			.map((result) => ({
				tool: result.tool,
				reviewRunId: result.proposal_review_run_id as string
			}));
		const citations = (record.tool_results ?? []).flatMap((result) =>
			harvestCitations(result.tool, result.output)
		);

		return {
			kind: 'assistant',
			id: record.id,
			content: record.content,
			createdAt: record.created_at,
			tools,
			proposals,
			citations: [...new Set(citations)],
			tokens: readTokenCounts(record.metadata)
		};
	}

	function readTokenCounts(
		metadata: Record<string, unknown> | null
	): { input: number; output: number } | null {
		if (!metadata) return null;
		const input = metadata['input_tokens'];
		const output = metadata['output_tokens'];
		if (typeof input !== 'number' || typeof output !== 'number') return null;
		return { input, output };
	}

	function startNewChat(): void {
		if (isStreaming) return;
		activeConversationId = null;
		turns = [];
		threadError = null;
		streamError = null;
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

	function handleStreamEvent(event: AssistantChatStreamEvent, turn: ChatTurn): void {
		const assistantTurn = turns.find((t) => t.id === turn.id);
		if (!assistantTurn || assistantTurn.kind !== 'assistant') return;
		switch (event.event) {
			case 'token':
				assistantTurn.content += event.delta;
				break;
			case 'tool_start':
				assistantTurn.tools = [
					...assistantTurn.tools,
					{
						toolCallId: event.tool_call_id,
						tool: event.tool,
						args: event.args,
						status: 'running',
						output: null
					}
				];
				break;
			case 'tool_complete': {
				assistantTurn.tools = assistantTurn.tools.map((toolCall) =>
					toolCall.toolCallId === event.tool_call_id
						? { ...toolCall, status: 'completed', output: event.output }
						: toolCall
				);
				assistantTurn.citations = [
					...new Set([
						...assistantTurn.citations,
						...harvestCitations(event.tool, event.output)
					])
				].slice(0, 8);
				break;
			}
			case 'proposal_created':
				assistantTurn.proposals = [
					...assistantTurn.proposals,
					{ tool: event.tool, reviewRunId: event.review_run_id }
				];
				break;
			case 'done':
				assistantTurn.tokens = { input: event.input_tokens, output: event.output_tokens };
				break;
			case 'error':
				streamError = event.message;
				break;
		}
		turns = [...turns];
	}

	async function send(): Promise<void> {
		const message = draft.trim();
		if (!message || isStreaming) return;

		streamError = null;
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
		const assistantTurn = emptyAssistantTurn();
		turns = [...turns, userTurn, assistantTurn];
		streamingTurnId = assistantTurn.id;

		try {
			await streamAssistantChat(
				projectId,
				{ conversation_id: conversationId, message },
				(event) => handleStreamEvent(event, assistantTurn)
			);
		} catch (error: unknown) {
			streamError =
				error instanceof Error
					? error.message
					: 'The assistant stream failed unexpectedly.';
		} finally {
			streamingTurnId = null;
			void refreshConversations(false);
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
	<aside
		class="absolute inset-y-0 left-0 z-10 flex w-72 flex-col border-r border-border/70 bg-background transition-transform duration-150 md:static md:translate-x-0 {sidebarOpen
			? 'translate-x-0 shadow-xl md:shadow-none'
			: '-translate-x-full'}"
		aria-label="Assistant conversations"
	>
		<div class="flex items-center justify-between gap-2 border-b border-border/70 px-4 py-4">
			<p class="text-xs font-semibold tracking-[0.18em] text-muted-foreground uppercase">
				Conversations
			</p>
			<Button
				size="sm"
				variant="secondary"
				onclick={startNewChat}
				disabled={isStreaming}
				data-testid="assistant-new-chat"
			>
				<PlusIcon data-icon="inline-start" aria-hidden="true" />
				New Chat
			</Button>
		</div>
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
				<p class="px-3 py-2 text-sm text-muted-foreground">
					No conversations yet. Start a new chat to begin.
				</p>
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
	</aside>

	<section class="flex min-w-0 flex-1 flex-col">
		<header class="flex items-center gap-3 border-b border-border/70 px-4 py-3 sm:px-6">
			<Button
				variant="ghost"
				size="icon"
				class="md:hidden"
				aria-label="Toggle conversations"
				aria-expanded={sidebarOpen}
				onclick={() => (sidebarOpen = !sidebarOpen)}
			>
				<PanelLeftIcon aria-hidden="true" />
			</Button>
			<div class="min-w-0">
				<h1 class="truncate text-base font-semibold text-foreground">
					{activeConversation?.title ?? 'New conversation'}
				</h1>
				<p class="text-xs text-muted-foreground">
					Ask about this project's evidence. Proposals always wait for human review.
				</p>
			</div>
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
								<Bubble.Root variant="default" class="max-w-[85%] px-4 py-2.5">
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
											<Bubble.Root variant="muted" class="px-4 py-2.5">
												<p
													class="text-sm leading-relaxed break-words whitespace-pre-wrap"
												>
													{turn.content}
												</p>
											</Bubble.Root>
										{/if}
										{#each turn.tools as toolCall (toolCall.toolCallId)}
											<ToolCallCard
												toolName={toolCall.tool}
												arguments={toolCall.args ?? {}}
												status={toolCall.status}
												outputSummary={toolCall.status === 'completed'
													? summarizedOutput(toolCall.output)
													: undefined}
											/>
										{/each}
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
										{#if turn.id === streamingTurnId}
											<Marker.Root data-testid="assistant-streaming">
												<Marker.Content>{markerMessage}</Marker.Content>
											</Marker.Root>
										{/if}
									</Bubble.Group>
									{#if turn.citations.length > 0}
										<div class="flex flex-wrap gap-1.5">
											{#each turn.citations as citation (citation)}
												<Attachment.Root>
													<Attachment.Preview>
														<FileTextIcon
															class="size-3"
															aria-hidden="true"
														/>
													</Attachment.Preview>
													<Attachment.Name>{citation}</Attachment.Name>
												</Attachment.Root>
											{/each}
										</div>
									{/if}
									{#if turn.tokens}
										<Message.Footer class="text-[11px]">
											<span>{turn.tokens.input} tokens in</span>
											<span>·</span>
											<span>{turn.tokens.output} tokens out</span>
										</Message.Footer>
									{/if}
								</div>
							</Message.Root>
						{/if}
					{/each}
					{#if turns.length === 0 && !threadError}
						<div
							class="flex flex-col items-center gap-2 rounded-xl border border-dashed border-border/70 px-6 py-10 text-center"
							data-testid="assistant-empty-state"
						>
							<BotIcon class="size-6 text-muted-foreground" aria-hidden="true" />
							<p class="text-sm font-medium text-foreground">
								Ask this project's evidence anything
							</p>
							<p class="max-w-md text-sm text-muted-foreground">
								The assistant can read the protocol, search reports and documents,
								and queue review proposals. It never changes scientific records
								directly.
							</p>
						</div>
					{/if}
				</Message.Group>
			</div>
		</ScrollArea>

		<div class="border-t border-border/70 px-4 py-3 sm:px-6">
			<div class="mx-auto w-full max-w-3xl">
				{#if streamError}
					<Alert.Root
						variant="destructive"
						role="alert"
						class="mb-3"
						data-testid="assistant-stream-error"
					>
						<Alert.Title>The assistant turn failed</Alert.Title>
						<Alert.Description>{streamError}</Alert.Description>
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
						disabled={isStreaming}
						data-testid="assistant-thread-input"
					/>
					<Button
						size="icon"
						class="size-9 shrink-0"
						aria-label="Send message"
						onclick={() => void send()}
						disabled={isStreaming || draft.trim().length === 0}
						data-testid="assistant-send"
					>
						{#if isStreaming}
							<Spinner />
						{:else}
							<SendHorizontalIcon aria-hidden="true" />
						{/if}
					</Button>
				</InputGroup.Root>
				<p class="mt-2 text-center text-[11px] text-muted-foreground">
					Enter to send · Shift + Enter for a new line
				</p>
			</div>
		</div>
	</section>
</div>
