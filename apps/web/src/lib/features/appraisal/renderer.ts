import type {
	AppraisalAnswerSchemaDto,
	AppraisalQuestionDto,
	CompleteAppraisalRequestResponses
} from '$lib/api/generated/models';

export type ResponseValue = string | boolean | number;

export function defaultResponse(schema: AppraisalAnswerSchemaDto): ResponseValue | undefined {
	switch (schema.kind) {
		case 'enum':
			return schema.options[0]?.value;
		case 'boolean':
			return undefined;
		case 'scale':
			return undefined;
		case 'text':
			return undefined;
	}
}

export function responseIsComplete(
	question: AppraisalQuestionDto,
	responses: CompleteAppraisalRequestResponses
): boolean {
	const value = responses[question.id];
	if (!question.required) return true;
	if (value === undefined || value === null) return false;
	return typeof value !== 'string' || value.trim().length > 0;
}

export function responseLabel(
	schema: AppraisalAnswerSchemaDto,
	value: unknown
): string | undefined {
	if (schema.kind !== 'enum' || typeof value !== 'string') return undefined;
	return schema.options.find((option) => option.value === value)?.label;
}
