/**
 * Pure rules for the automation builder's header and canvas: why a control is
 * unavailable right now, and where a new block can go without covering one
 * that is already on the canvas.
 */

export type BuilderAction = 'add' | 'save' | 'check' | 'test' | 'publish' | 'switch';

export interface BuilderControlState {
	/** Viewing a past run: nothing can be edited. */
	readOnly: boolean;
	/** A save, check, test, publish or on/off request is in flight. */
	busy: boolean;
	/** Changed elsewhere; the last edits were not saved. */
	conflict: boolean;
	/** Number of blocks on the canvas (notes do not count). */
	blockCount: number;
	/** Published at least once. */
	published: boolean;
}

const PAST_RUN = 'You are looking at a past run. Go back to editing to change anything.';
const CONFLICT = 'This automation was changed somewhere else. Load the latest version first.';
const BUSY = 'Wait for the current step to finish.';
const NO_BLOCKS = 'Add a block first. A test needs a starting point.';
/** Why the on/off switch is off until the automation has been published. */
export const PUBLISH_FIRST = 'Publish first. You can turn it on after that.';

/**
 * Plain-language reason an action cannot be used right now, or `null` when it
 * can. Shown as helper text next to the control and as its tooltip.
 */
export function unavailableReason(
	action: BuilderAction,
	state: BuilderControlState
): string | null {
	if (state.readOnly && action !== 'switch') return PAST_RUN;
	if ((action === 'save' || action === 'publish') && state.conflict) return CONFLICT;
	// Adding a block is harmless while a save runs, so only the others wait.
	if (state.busy && action !== 'add') return BUSY;
	if (action === 'test' && state.blockCount === 0) return NO_BLOCKS;
	if (action === 'switch' && !state.published) return PUBLISH_FIRST;
	return null;
}

/**
 * The one reason worth showing as helper text under the header, or `null`.
 * Work in progress ("busy") is not a reason to explain.
 */
export function headerHelp(state: BuilderControlState, manual: boolean): string | null {
	if (state.busy) return null;
	if (unavailableReason('publish', state)) return unavailableReason('publish', state);
	if (manual) return state.published ? null : 'Publish first. Then you can run it by hand.';
	return unavailableReason('switch', state) ?? unavailableReason('test', state);
}

export interface Box {
	x: number;
	y: number;
	width: number;
	height: number;
}

/**
 * The position nearest to `wanted` where a block of `size` fits without
 * touching any box in `taken`. Used so a block added from the middle of the
 * screen does not land on top of another one.
 */
export function freeSpot(
	wanted: { x: number; y: number },
	taken: Box[],
	size: { width: number; height: number },
	step = 40,
	gap = 20
): { x: number; y: number } {
	const clear = (x: number, y: number) =>
		taken.every(
			(box) =>
				x + size.width + gap <= box.x ||
				box.x + box.width + gap <= x ||
				y + size.height + gap <= box.y ||
				box.y + box.height + gap <= y
		);
	if (clear(wanted.x, wanted.y)) return wanted;
	const offsets: { dx: number; dy: number }[] = [];
	for (let i = -8; i <= 8; i += 1) {
		for (let j = -8; j <= 8; j += 1) {
			if (i === 0 && j === 0) continue;
			offsets.push({ dx: i * step, dy: j * step });
		}
	}
	offsets.sort(
		(a, b) => Math.hypot(a.dx, a.dy) - Math.hypot(b.dx, b.dy) || a.dy - b.dy || a.dx - b.dx
	);
	for (const { dx, dy } of offsets) {
		const x = wanted.x + dx;
		const y = wanted.y + dy;
		if (clear(x, y)) return { x, y };
	}
	return wanted;
}
