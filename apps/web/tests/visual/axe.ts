import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

type AxeViolation = Awaited<ReturnType<AxeBuilder['analyze']>>['violations'][number];

/** Run only WCAG violations that are serious or critical for this shell. */
export async function runSeriousCriticalAxe(page: Page): Promise<AxeViolation[]> {
	const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
	return results.violations.filter(
		(violation) => violation.impact === 'serious' || violation.impact === 'critical'
	);
}
