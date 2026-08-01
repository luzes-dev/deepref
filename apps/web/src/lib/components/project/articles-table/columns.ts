import type { ColumnDef } from '@tanstack/table-core';
import type { ArticleDto } from '$lib/api/generated/models';
import { renderComponent } from '$lib/components/ui/data-table';
import ArticleDataTableCheckbox from './ArticleDataTableCheckbox.svelte';
import ArticleDataTableColumnHeader from './ArticleDataTableColumnHeader.svelte';
import ArticleDataTableRowActions from './ArticleDataTableRowActions.svelte';
import ArticleTitleCell from './ArticleTitleCell.svelte';

function yearLabel(year: ArticleDto['issued_year']) {
	return String(year ?? 'No year');
}

function articleMatchesTerm(article: ArticleDto, term: string) {
	const title = article.title?.toLowerCase();
	const doi = article.doi.toLowerCase();
	return doi.includes(term) || Boolean(title?.includes(term));
}

function hasYearRange(value: unknown): value is unknown[] {
	return Array.isArray(value) && value.length >= 2;
}

function isWithinYearRange(year: number, range: unknown[]) {
	const [min, max] = range.map(Number);
	return year >= min && year <= max;
}

function articleMatchesYearRange(year: ArticleDto['issued_year'], value: unknown) {
	if (!hasYearRange(value)) return true;

	return typeof year === 'number' && isWithinYearRange(year, value);
}

export function createArticleColumns({
	openArticle,
	selectedArticle
}: {
	openArticle: (doiKey: string) => void;
	selectedArticle?: string;
}): ColumnDef<ArticleDto>[] {
	return [
		{
			id: 'select',
			header: ({ table }) =>
				renderComponent(ArticleDataTableCheckbox, {
					checked: table.getIsAllPageRowsSelected(),
					indeterminate:
						table.getIsSomePageRowsSelected() && !table.getIsAllPageRowsSelected(),
					onCheckedChange: (value) => table.toggleAllPageRowsSelected(Boolean(value)),
					'aria-label': 'Select all'
				}),
			cell: ({ row }) =>
				renderComponent(ArticleDataTableCheckbox, {
					checked: row.getIsSelected(),
					onCheckedChange: (value) => row.toggleSelected(Boolean(value)),
					'aria-label': 'Select row'
				}),
			enableSorting: false,
			enableHiding: false
		},
		{
			id: 'title',
			accessorFn: (article) => article.title ?? article.doi,
			header: ({ column }) =>
				renderComponent(ArticleDataTableColumnHeader<ArticleDto, unknown>, {
					column,
					title: 'Article'
				}),
			cell: ({ row }) =>
				renderComponent(ArticleTitleCell, {
					article: row.original,
					selected: selectedArticle === row.original.doi_key,
					openArticle
				}),
			filterFn: (row, _columnId, value) => {
				const term = String(value ?? '')
					.trim()
					.toLowerCase();
				if (!term) return true;
				return articleMatchesTerm(row.original, term);
			},
			enableHiding: false
		},
		{
			id: 'type',
			accessorFn: (article) => article.type ?? 'Unknown',
			header: ({ column }) =>
				renderComponent(ArticleDataTableColumnHeader<ArticleDto, unknown>, {
					column,
					title: 'Type'
				}),
			cell: ({ row }) => row.original.type ?? 'Unknown',
			filterFn: (row, id, value) => (value as string[]).includes(row.getValue(id))
		},
		{
			id: 'issued_year',
			accessorFn: (article) => yearLabel(article.issued_year),
			header: ({ column }) =>
				renderComponent(ArticleDataTableColumnHeader<ArticleDto, unknown>, {
					column,
					title: 'Year'
				}),
			cell: ({ row }) => row.original.issued_year ?? '-',
			filterFn: (row, _id, value) => articleMatchesYearRange(row.original.issued_year, value),
			sortingFn: (rowA, rowB) =>
				(rowA.original.issued_year ?? 0) - (rowB.original.issued_year ?? 0)
		},
		{
			accessorKey: 'total_citations',
			header: ({ column }) =>
				renderComponent(ArticleDataTableColumnHeader<ArticleDto, unknown>, {
					column,
					title: 'Total'
				})
		},
		{
			accessorKey: 'internal_citations',
			header: ({ column }) =>
				renderComponent(ArticleDataTableColumnHeader<ArticleDto, unknown>, {
					column,
					title: 'Internal'
				}),
			filterFn: (row, id, value) => row.getValue<number>(id) >= Number(value ?? 0)
		},
		{
			accessorKey: 'outbound_internal_references',
			header: ({ column }) =>
				renderComponent(ArticleDataTableColumnHeader<ArticleDto, unknown>, {
					column,
					title: 'Outbound'
				})
		},
		{
			accessorKey: 'rank_score',
			header: ({ column }) =>
				renderComponent(ArticleDataTableColumnHeader<ArticleDto, unknown>, {
					column,
					title: 'Rank'
				}),
			cell: ({ row }) => row.original.rank_score.toFixed(2)
		},
		{
			id: 'actions',
			cell: ({ row }) => renderComponent(ArticleDataTableRowActions, { row, openArticle }),
			enableSorting: false,
			enableHiding: false
		}
	];
}
