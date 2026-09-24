import Root, {
	bubbleVariants,
	type BubbleVariant,
	type BubbleSize,
} from "./bubble-root.svelte";
import Content from "./bubble-content.svelte";
import Group from "./bubble-group.svelte";
import Reactions from "./bubble-reactions.svelte";

export {
	Root,
	Content,
	Group,
	Reactions,
	bubbleVariants,
	type BubbleVariant,
	type BubbleSize,
	//
	Root as Bubble,
	Content as BubbleContent,
	Group as BubbleGroup,
	Reactions as BubbleReactions,
};
