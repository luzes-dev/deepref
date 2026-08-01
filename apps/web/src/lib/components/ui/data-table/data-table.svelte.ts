import {
	createTable,
	type RowData,
	type TableOptions,
	type TableOptionsResolved,
	type TableState
} from '@tanstack/table-core';

export function createSvelteTable<TData extends RowData>(options: TableOptions<TData>) {
	const resolvedOptions: TableOptionsResolved<TData> = mergeObjects(
		{
			state: {},
			onStateChange() {},
			renderFallbackValue: null,
			mergeOptions: (
				defaultOptions: TableOptions<TData>,
				options: Partial<TableOptions<TData>>
			) => mergeObjects(defaultOptions, options)
		},
		options
	);

	const table = createTable(resolvedOptions);
	let state = $state<Partial<TableState>>(table.initialState);

	function updateOptions() {
		table.setOptions((previous) =>
			mergeObjects(previous, options, {
				state: mergeObjects(state, options.state || {}),
				// eslint-disable-next-line @typescript-eslint/no-explicit-any
				onStateChange: (updater: any) => {
					if (typeof updater === 'function') {
						state = updater(state);
					} else {
						state = mergeObjects(state, updater);
					}

					options.onStateChange?.(updater);
				}
			})
		);
	}

	updateOptions();

	$effect.pre(() => {
		updateOptions();
	});

	return table;
}

export function mergeObjects<T>(source: T): T;
export function mergeObjects<T, U>(source: T, source1: U): T & U;
export function mergeObjects<T, U, V>(source: T, source1: U, source2: V): T & U & V;
export function mergeObjects<T, U, V, W>(
	source: T,
	source1: U,
	source2: V,
	source3: W
): T & U & V & W;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function mergeObjects(...sources: any): any {
	const target = {};
	for (let i = 0; i < sources.length; i += 1) {
		let source = sources[i];
		if (typeof source === 'function') source = source();
		if (source) {
			const descriptors = Object.getOwnPropertyDescriptors(source);
			for (const key in descriptors) {
				if (key in target) continue;
				Object.defineProperty(target, key, {
					enumerable: true,
					get() {
						for (let i = sources.length - 1; i >= 0; i -= 1) {
							let s = sources[i];
							if (typeof s === 'function') s = s();
							const value = (s || {})[key];
							if (value !== undefined) return value;
						}
					}
				});
			}
		}
	}
	return target;
}
