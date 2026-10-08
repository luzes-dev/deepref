import { toast } from 'svelte-sonner';
import { plainNotificationText } from './notification-copy';

export type NotifyVariant = 'success' | 'error' | 'warning' | 'info';

type ExternalToast = Parameters<typeof toast.success>[1];

export function messageFromError(error: unknown, fallback: string): string {
	if (error instanceof Error && error.message.trim() !== '') return error.message;
	return fallback;
}

export function notifySuccess(title: string, description?: string): string | number {
	return toast.success(title, description ? { description } : undefined);
}

export function notifyError(
	title: string,
	error?: unknown,
	fallback = 'The request failed. Please try again.',
	options?: ExternalToast
): string | number {
	const description = error === undefined ? fallback : messageFromError(error, fallback);
	return toast.error(title, { description, ...options });
}

export function notifyWarning(
	title: string,
	description?: string,
	options?: ExternalToast
): string | number {
	return toast.warning(title, { description, ...options });
}

export function notifyInfo(title: string, description?: string): string | number {
	return toast.info(title, description ? { description } : undefined);
}

export function notifyNotification(notification: {
	severity: string;
	title: string;
	body?: string | null;
}): string | number {
	// Same plain-language copy as the bell: decoded titles, translated failures.
	const copy = plainNotificationText(notification);
	const description = copy.body ?? undefined;
	switch (notification.severity) {
		case 'success':
			return notifySuccess(copy.title, description);
		case 'warning':
			return notifyWarning(copy.title, description);
		case 'error':
			return toast.error(copy.title, { description });
		default:
			return notifyInfo(copy.title, description);
	}
}
