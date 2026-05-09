import OpenAI from 'openai';
import type { Stream } from 'openai/streaming';
import type {
  ChatCompletion,
  ChatCompletionChunk,
  ChatCompletionMessageParam,
} from 'openai/resources/chat/completions';
import type { CreateEmbeddingResponse } from 'openai/resources/embeddings';
import type { ImagesResponse } from 'openai/resources/images';
import { configService } from '../../services/config.service.js';

export class ByocError extends Error {
  status: number | undefined;
  data: unknown;
  constructor(message: string, status: number | undefined, data: unknown) {
    super(message);
    this.name = 'ByocError';
    this.status = status;
    this.data = data;
  }
}

function client(): OpenAI {
  const cfg = configService.value;
  return new OpenAI({
    apiKey: cfg.gatewayBearer || 'unused',
    baseURL: cfg.byocGatewayUrl,
    dangerouslyAllowBrowser: true,
  });
}

function normalize(err: unknown): never {
  const e = err as { error?: { message?: string }; message?: string; status?: number };
  const message = e.error?.message ?? e.message ?? 'Request failed.';
  throw new ByocError(message, e.status, err);
}

export interface ByocChatPayload {
  model: string;
  messages: ChatCompletionMessageParam[];
  temperature: number;
  max_tokens: number;
}

export const openaiSdk = {
  async chat(payload: ByocChatPayload): Promise<ChatCompletion> {
    try {
      return await client().chat.completions.create({ ...payload, stream: false });
    } catch (err) {
      normalize(err);
    }
  },

  async chatStream(payload: ByocChatPayload): Promise<Stream<ChatCompletionChunk>> {
    try {
      return await client().chat.completions.create({ ...payload, stream: true });
    } catch (err) {
      normalize(err);
    }
  },

  async images(payload: {
    model: string;
    prompt: string;
    size: '1024x1024' | '1024x1792' | '1792x1024';
    n: number;
  }): Promise<ImagesResponse> {
    try {
      return await client().images.generate({ ...payload, response_format: 'b64_json' });
    } catch (err) {
      normalize(err);
    }
  },

  async embeddings(payload: { model: string; input: string }): Promise<CreateEmbeddingResponse> {
    try {
      return await client().embeddings.create(payload);
    } catch (err) {
      normalize(err);
    }
  },
};

/** Strips the Llama-style header tokens some BYOC backends emit. */
export function stripHeader(value: unknown): string {
  if (typeof value === 'string') {
    return value.replace(/<\|start_header_id\|>assistant<\|end_header_id\|>/g, '');
  }
  if (Array.isArray(value)) return value.map((v) => stripHeader(v)).join('');
  if (value && typeof value === 'object') {
    const o = value as { text?: unknown; content?: unknown };
    if (typeof o.text === 'string') return stripHeader(o.text);
    if (typeof o.content === 'string') return stripHeader(o.content);
  }
  return '';
}

/** Friendly error message for common BYOC HTTP statuses. */
export function byocErrorMessage(err: unknown): string {
  if (err instanceof ByocError) {
    if (err.status === 401 || err.status === 403)
      return 'Unauthorized request. Check your configured bearer token.';
    if (err.status === 429) return 'Rate limited. Please retry shortly.';
    return err.message;
  }
  return err instanceof Error ? err.message : String(err);
}
