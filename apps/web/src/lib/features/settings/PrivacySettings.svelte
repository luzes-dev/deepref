<script lang="ts">
	import { archiveCoverage } from './export-coverage.js';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Button } from '@deepref/ui/button';
	import {
		createGetAccountExportStatus,
		createRequestAccountExport,
		createCancelAccountExport,
		getGetAccountExportStatusQueryKey,
		getDownloadAccountExportUrl
	} from '#lib/api/generated/auth/auth.js';
	const client = useQueryClient();
	const status = createGetAccountExportStatus(() => ({ query: { retry: false, staleTime: 0 } }));
	const refresh = () =>
		client.invalidateQueries({ queryKey: getGetAccountExportStatusQueryKey() });
	const request = createRequestAccountExport(() => ({ mutation: { onSuccess: refresh } }));
	const cancel = createCancelAccountExport(() => ({ mutation: { onSuccess: refresh } }));
	const record = $derived(status.data?.data);
	const coverage = $derived(archiveCoverage(record?.scope_manifest));
	const pending = $derived(request.isPending || cancel.isPending);
	const active = $derived(
		record && ['requested', 'processing', 'completed'].includes(record.status)
	);
</script>

<section class="flex flex-col gap-4" aria-labelledby="privacy-title">
	<div>
		<h3 id="privacy-title" class="text-base font-semibold">Download my data</h3>
		<p class="text-sm text-muted-foreground">
			A ZIP of your account, workspaces you own (including co-owned workspaces), scientific
			records, local billing projection and uploaded files. Membership alone does not include
			another owner's workspace.
		</p>
	</div>
	<details class="disclosure">
		<summary class="cursor-pointer text-sm font-medium">Coverage and limits</summary>
		<div class="flex flex-col gap-2 pt-3 text-sm text-muted-foreground">
			<p>
				New exports include original documents and workflow files, parsed pages/blocks,
				study relationships, screening, appraisal, extraction, and review automation output.
				The archive manifest lists the exact sections and integrity hashes are verified
				before publication.
			</p>
			<p>
				Excludes sessions/tokens, API-key secrets and verifiers, raw connector/provider
				payloads, administrative/security audit, internal financial notes, executable
				workflow configuration and node bodies, prepared model requests, and reconstructable
				embeddings/graph caches.
			</p>
			<p>
				Per attempt: 50 owned workspaces, 20,000 rows, 2,000 files, 32 MiB JSON, 512 MiB
				source bytes and 10 minutes. A limit or integrity failure fails the request; covered
				rows are never silently dropped. Archives expire seven days after collection and
				require current ownership to download.
			</p>
		</div>
	</details>
	{#if status.isPending}<p role="status">Loading export status…</p>
	{:else if status.isError}<p role="alert">
			Export status is unavailable. Refresh before requesting another archive.
		</p>
	{:else if record}
		<div class="flex flex-col gap-2" role="status" aria-label="Export status">
			<p class="text-sm font-medium">Export {record.status}</p>
			<p class="text-xs text-muted-foreground">Requested {record.requested_at}</p>
			{#if record.retained_until}<p class="text-xs text-muted-foreground">
					Retained until {record.retained_until}
				</p>{/if}{#if record.failure_reason}<p class="text-sm">
					The export failed ({record.failure_reason}). No complete archive is available.
				</p>{/if}{#if record.sha256}<p class="text-xs break-all text-muted-foreground">
					SHA-256: {record.sha256}
				</p>{/if}{#if record.size_bytes != null}<p class="text-xs text-muted-foreground">
					{record.size_bytes.toLocaleString()} bytes
				</p>{/if}
		</div>
	{:else}<p class="text-sm text-muted-foreground">You have not requested an export.</p>{/if}
	{#if record?.status === 'completed'}
		<details class="disclosure">
			<summary class="cursor-pointer text-sm font-medium"
				>Exact coverage of this archive</summary
			>
			{#if coverage}<div class="flex flex-col gap-2 pt-3 text-sm">
					<p>Sections: {coverage.sections.join(', ')}</p>
					<p>Exclusions: {coverage.exclusions.join('; ')}</p>
				</div>{:else}<p class="pt-3 text-sm text-muted-foreground">
					Coverage details are unavailable in this status. Inspect manifest.json inside
					the archive before treating it as complete for your needs.
				</p>{/if}
		</details>
	{/if}
	{#if request.isError || cancel.isError}<p role="alert">
			The export change failed. Refresh its status before retrying. Export availability may
			require operator activation.
		</p>{/if}
	<div class="flex flex-wrap gap-2">
		<Button
			disabled={pending ||
				status.isPending ||
				status.isError ||
				Boolean(record && ['requested', 'processing'].includes(record.status))}
			onclick={() => request.mutate()}>Request data export</Button
		>
		<Button
			variant="outline"
			disabled={status.isFetching || pending}
			onclick={() => void status.refetch()}>Refresh export status</Button
		>
		{#if record?.status === 'completed' && record.retained_until && Date.parse(record.retained_until) > Date.now()}<Button
				variant="outline"
				href={getDownloadAccountExportUrl(record.id)}
				download>Download ZIP</Button
			>{/if}
		{#if active && record}<Button
				variant="outline"
				disabled={pending}
				onclick={() => cancel.mutate({ id: record.id })}
				>{record.status === 'completed' ? 'Discard archive' : 'Cancel export'}</Button
			>{/if}
	</div>
</section>
