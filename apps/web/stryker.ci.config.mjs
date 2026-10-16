import { globSync, readFileSync, statSync } from 'node:fs';
import base from './stryker.config.json' with { type: 'json' };

const shardCount = Number(process.env.STRYKER_SHARD_COUNT);
const shardIndex = Number(process.env.STRYKER_SHARD_INDEX);
if (
	!Number.isInteger(shardCount) ||
	shardCount < 1 ||
	!Number.isInteger(shardIndex) ||
	shardIndex < 0 ||
	shardIndex >= shardCount
) {
	throw new Error('Set STRYKER_SHARD_INDEX within STRYKER_SHARD_COUNT.');
}

const files = globSync(
	base.mutate.filter((pattern) => !pattern.startsWith('!')),
	{
		exclude: base.mutate
			.filter((pattern) => pattern.startsWith('!'))
			.map((pattern) => pattern.slice(1))
	}
).filter((file) => statSync(file).isFile());
const weighted = files.map((file) => ({ file, size: readFileSync(file).length }));
weighted.sort((left, right) => right.size - left.size || left.file.localeCompare(right.file));
const shards = Array.from({ length: shardCount }, () => ({ files: [], size: 0 }));
for (const { file, size } of weighted) {
	const shard = shards.reduce((lightest, candidate) =>
		candidate.size < lightest.size ? candidate : lightest
	);
	shard.files.push(file);
	shard.size += size;
}

export default {
	...base,
	mutate: shards[shardIndex].files,
	concurrency: 2
};
