import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

export type WithoutChild<T> = T extends { child?: unknown } ? Omit<T, 'child'> : T;
export type WithoutChildren<T> = T extends { children?: unknown } ? Omit<T, 'children'> : T;
// fallow-ignore-next-line unused-type -- Utility type exported for shadcn-svelte component consumers
export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>;
// fallow-ignore-next-line unused-type -- Utility type exported for shadcn-svelte component consumers
export type WithElementRef<T, U extends HTMLElement = HTMLElement> = T & { ref?: U | null };
