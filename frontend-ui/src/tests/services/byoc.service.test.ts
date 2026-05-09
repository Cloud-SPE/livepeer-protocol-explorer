import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/openai-sdk.js', async (importOriginal) => {
  const actual = (await importOriginal()) as Record<string, unknown>;
  return {
    ...actual,
    openaiSdk: {
      chat: vi.fn(),
      chatStream: vi.fn(),
      images: vi.fn(),
      embeddings: vi.fn(),
    },
  };
});

const { byocService } = await import('../../services/byoc.service.js');
const { openaiSdk, ByocError } = await import('../../lib/sources/openai-sdk.js');

beforeEach(() => {
  vi.mocked(openaiSdk.chat).mockReset();
  vi.mocked(openaiSdk.chatStream).mockReset();
  vi.mocked(openaiSdk.images).mockReset();
  vi.mocked(openaiSdk.embeddings).mockReset();
  byocService.reset();
});

const chatPayload = {
  model: 'm1',
  messages: [{ role: 'user' as const, content: 'hi' }],
  temperature: 0.7,
  max_tokens: 256,
};

describe('byocService.runChat (non-stream)', () => {
  it('captures content and reasoning', async () => {
    vi.mocked(openaiSdk.chat).mockResolvedValueOnce({
      choices: [{ message: { content: 'hello', reasoning: 'thinking' } }],
    } as unknown as Awaited<ReturnType<typeof openaiSdk.chat>>);
    await byocService.runChat(chatPayload, { stream: false });
    expect(byocService.chat.output).toBe('hello');
    expect(byocService.chat.reasoning).toBe('thinking');
    expect(byocService.chat.error).toBeNull();
  });

  it('records errors via byocErrorMessage', async () => {
    vi.mocked(openaiSdk.chat).mockRejectedValueOnce(new ByocError('boom', 401, {}));
    await byocService.runChat(chatPayload, { stream: false });
    expect(byocService.chat.error).toBe('Unauthorized request. Check your configured bearer token.');
  });
});

describe('byocService.runChat (stream)', () => {
  it('accumulates deltas from the async iterator', async () => {
    vi.mocked(openaiSdk.chatStream).mockResolvedValueOnce(
      (async function* () {
        yield { choices: [{ delta: { content: 'he' } }] };
        yield { choices: [{ delta: { content: 'llo' } }] };
        yield { choices: [{ delta: { reasoning: 'why' } }] };
      })() as unknown as Awaited<ReturnType<typeof openaiSdk.chatStream>>,
    );
    await byocService.runChat(chatPayload, { stream: true });
    expect(byocService.chat.output).toBe('hello');
    expect(byocService.chat.reasoning).toBe('why');
    expect(byocService.chat.loading).toBe(false);
  });
});

describe('byocService.runImages', () => {
  it('inflates b64_json into data: URLs', async () => {
    vi.mocked(openaiSdk.images).mockResolvedValueOnce({
      data: [{ b64_json: 'AAAA' }, { b64_json: 'BBBB' }],
    } as unknown as Awaited<ReturnType<typeof openaiSdk.images>>);
    await byocService.runImages({ model: 'm', prompt: 'p', size: '1024x1024', n: 2 });
    expect(byocService.images.images).toEqual([
      'data:image/png;base64,AAAA',
      'data:image/png;base64,BBBB',
    ]);
    expect(byocService.images.error).toBeNull();
  });

  it('falls back to URL when no b64_json', async () => {
    vi.mocked(openaiSdk.images).mockResolvedValueOnce({
      data: [{ url: 'https://example.com/x.png' }],
    } as unknown as Awaited<ReturnType<typeof openaiSdk.images>>);
    await byocService.runImages({ model: 'm', prompt: 'p', size: '1024x1024', n: 1 });
    expect(byocService.images.images).toEqual(['https://example.com/x.png']);
  });
});

describe('byocService.runEmbeddings', () => {
  it('captures embedding + dims when shaped as data[]', async () => {
    vi.mocked(openaiSdk.embeddings).mockResolvedValueOnce({
      data: [{ embedding: [0.1, 0.2, 0.3] }],
    } as unknown as Awaited<ReturnType<typeof openaiSdk.embeddings>>);
    await byocService.runEmbeddings({ model: 'm', input: 'hi' });
    expect(byocService.embeddings.embedding).toEqual([0.1, 0.2, 0.3]);
    expect(byocService.embeddings.dims).toBe(3);
  });

  it('falls back to top-level embedding shape', async () => {
    vi.mocked(openaiSdk.embeddings).mockResolvedValueOnce({
      embedding: [1, 2],
    } as unknown as Awaited<ReturnType<typeof openaiSdk.embeddings>>);
    await byocService.runEmbeddings({ model: 'm', input: 'hi' });
    expect(byocService.embeddings.embedding).toEqual([1, 2]);
    expect(byocService.embeddings.dims).toBe(2);
  });

  it('records errors', async () => {
    vi.mocked(openaiSdk.embeddings).mockRejectedValueOnce(new ByocError('rate', 429, {}));
    await byocService.runEmbeddings({ model: 'm', input: 'hi' });
    expect(byocService.embeddings.error).toBe('Rate limited. Please retry shortly.');
  });
});
