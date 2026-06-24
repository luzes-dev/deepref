export function statusVariant(status: string): 'default' | 'secondary' | 'destructive' | 'outline' {
	if (['failed', 'not_found'].includes(status)) return 'destructive';
	if (['completed', 'fetched'].includes(status)) return 'default';
	if (['running', 'fetching'].includes(status)) return 'secondary';
	return 'outline';
}

export function shouldPollIngestion(status?: string): number | false {
	return status === undefined || status === 'queued' || status === 'running' ? 2_000 : false;
}
