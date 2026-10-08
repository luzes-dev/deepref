export const MAX_MONTHLY_BUDGET_USD = 100000;

export type BudgetInput = { ok: true; amount: number } | { ok: false; message: string };

/** Parses the monthly budget typed into Settings. Zero is valid and pauses the AI. */
export function parseBudgetInput(raw: string): BudgetInput {
	const trimmed = raw.trim();
	const amount = Number(trimmed);
	if (
		trimmed === '' ||
		!Number.isFinite(amount) ||
		amount < 0 ||
		amount > MAX_MONTHLY_BUDGET_USD
	) {
		return {
			ok: false,
			message: `Enter an amount between 0 and ${MAX_MONTHLY_BUDGET_USD.toLocaleString('en-US')} dollars.`
		};
	}
	return { ok: true, amount };
}

/** A zero budget means no AI call is allowed in the month. */
export function isAiPaused(monthlyBudgetUsd: number): boolean {
	return monthlyBudgetUsd === 0;
}
