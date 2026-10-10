import {
	columnFacetingFeature,
	columnFilteringFeature,
	columnVisibilityFeature,
	createFacetedRowModel,
	createFacetedUniqueValues,
	createFilteredRowModel,
	createPaginatedRowModel,
	createSortedRowModel,
	rowPaginationFeature,
	rowSortingFeature,
	tableFeatures
} from '@tanstack/svelte-table';

export const articleTableFeatures = tableFeatures({
	columnFacetingFeature,
	columnFilteringFeature,
	columnVisibilityFeature,
	rowPaginationFeature,
	rowSortingFeature,
	facetedRowModel: createFacetedRowModel(),
	facetedUniqueValues: createFacetedUniqueValues(),
	filteredRowModel: createFilteredRowModel(),
	paginatedRowModel: createPaginatedRowModel(),
	sortedRowModel: createSortedRowModel()
});

export type ArticleTableFeatures = typeof articleTableFeatures;
