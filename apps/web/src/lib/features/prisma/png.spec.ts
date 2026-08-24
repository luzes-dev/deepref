import { describe, expect, it } from 'vitest';
import { attachmentFilename, pngAttachmentFilename, svgDimensions } from './png';

describe('PRISMA export helpers', () => {
	it('uses the canonical SVG viewBox for raster dimensions', () => {
		expect(svgDimensions('<svg viewBox="0 0 1320 840"></svg>')).toEqual({
			width: 1320,
			height: 840
		});
	});

	it('falls back safely and extracts attachment filenames', () => {
		expect(svgDimensions('<svg></svg>')).toEqual({ width: 1320, height: 840 });
		expect(
			attachmentFilename(
				new Headers({ 'content-disposition': 'attachment; filename="deepref-prisma.svg"' }),
				'prisma.svg'
			)
		).toBe('deepref-prisma.svg');
		expect(
			pngAttachmentFilename(
				new Headers({ 'content-disposition': 'attachment; filename="deepref-prisma.svg"' }),
				'prisma.png'
			)
		).toBe('deepref-prisma.png');
		expect(pngAttachmentFilename(new Headers(), 'deepref-prisma.png')).toBe(
			'deepref-prisma.png'
		);
	});
});
