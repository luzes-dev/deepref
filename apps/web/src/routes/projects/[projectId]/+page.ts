import { redirect } from '@sveltejs/kit';

export function load({ params }: { params: { projectId: string } }) {
	redirect(307, `/projects/${params.projectId}/overview`);
}
