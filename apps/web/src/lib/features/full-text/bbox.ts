export type NormalizedBbox = {
	x: number;
	y: number;
	width: number;
	height: number;
};

export type PixelBbox = NormalizedBbox;

function bounded(value: number): number {
	return Math.min(1, Math.max(0, value));
}

export function normalizedBboxToPixels(
	bbox: NormalizedBbox | null | undefined,
	pageWidth: number,
	pageHeight: number
): PixelBbox | null {
	if (!bbox || !Number.isFinite(pageWidth) || !Number.isFinite(pageHeight)) return null;
	if (pageWidth <= 0 || pageHeight <= 0) return null;
	const x = bounded(bbox.x);
	const y = bounded(bbox.y);
	const right = bounded(bbox.x + Math.max(0, bbox.width));
	const bottom = bounded(bbox.y + Math.max(0, bbox.height));
	if (right <= x || bottom <= y) return null;
	return {
		x: x * pageWidth,
		y: y * pageHeight,
		width: (right - x) * pageWidth,
		height: (bottom - y) * pageHeight
	};
}
