import type {} from '@testing-library/jest-dom/vitest';
import * as matchers from '@testing-library/jest-dom/matchers';
import { expect } from 'vitest';
import { cleanup } from '@testing-library/svelte';
import { server } from './mocks/server';
import { beforeAll, afterEach, afterAll } from 'vitest';

expect.extend(matchers);

beforeAll(() => {
	server.listen({ onUnhandledRequest: 'error' });
});

afterEach(() => {
	cleanup();
	server.resetHandlers();
});

afterAll(() => {
	server.close();
});
