export type SvgDimensions = { width: number; height: number };

const DEFAULT_SVG_DIMENSIONS: SvgDimensions = { width: 1320, height: 840 };

export function svgDimensions(svg: string): SvgDimensions {
	const viewBox = svg.match(
		/\bviewBox\s*=\s*["']\s*[-\d.]+\s+[-\d.]+\s+([\d.]+)\s+([\d.]+)\s*["']/i
	);
	if (viewBox) {
		const width = Number(viewBox[1]);
		const height = Number(viewBox[2]);
		if (width > 0 && height > 0) return { width, height };
	}
	return DEFAULT_SVG_DIMENSIONS;
}

export function attachmentFilename(headers: Headers, fallback: string): string {
	const disposition = headers.get('content-disposition') ?? '';
	const match = disposition.match(/filename\s*=\s*"?([^";]+)"?/i);
	return match?.[1]?.trim() || fallback;
}

export function pngAttachmentFilename(headers: Headers, fallback: string): string {
	const filename = attachmentFilename(headers, fallback);
	return filename.toLowerCase().endsWith('.svg')
		? `${filename.slice(0, -'.svg'.length)}.png`
		: fallback;
}

export function downloadBlob(blob: Blob, filename: string): void {
	const url = URL.createObjectURL(blob);
	try {
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = filename;
		anchor.click();
	} finally {
		URL.revokeObjectURL(url);
	}
}

export async function downloadPrismaPng(svgBlob: Blob, filename: string): Promise<void> {
	const svgText = await svgBlob.text();
	const dimensions = svgDimensions(svgText);
	const svgUrl = URL.createObjectURL(new Blob([svgText], { type: 'image/svg+xml' }));
	try {
		const image = new Image();
		image.decoding = 'async';
		image.src = svgUrl;
		await new Promise<void>((resolve, reject) => {
			image.onload = () => resolve();
			image.onerror = () => reject(new Error('PRISMA SVG could not be rasterized'));
		});

		const scale = globalThis.devicePixelRatio || 1;
		const canvas = document.createElement('canvas');
		canvas.width = Math.ceil(dimensions.width * scale);
		canvas.height = Math.ceil(dimensions.height * scale);
		canvas.style.width = `${dimensions.width}px`;
		canvas.style.height = `${dimensions.height}px`;
		const context = canvas.getContext('2d');
		if (!context) throw new Error('PRISMA PNG canvas is unavailable');
		context.fillStyle = '#ffffff';
		context.fillRect(0, 0, canvas.width, canvas.height);
		context.drawImage(image, 0, 0, canvas.width, canvas.height);
		const png = await new Promise<Blob>((resolve, reject) => {
			canvas.toBlob((value) => {
				if (value) resolve(value);
				else reject(new Error('PRISMA PNG encoding failed'));
			}, 'image/png');
		});
		downloadBlob(png, filename);
	} finally {
		URL.revokeObjectURL(svgUrl);
	}
}
