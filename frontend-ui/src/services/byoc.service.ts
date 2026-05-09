import { BehaviorSubject, type Observable } from 'rxjs';
import {
  byocErrorMessage,
  openaiSdk,
  stripHeader,
  type ByocChatPayload,
} from '../lib/sources/openai-sdk.js';

interface ChatState {
  output: string;
  reasoning: string;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

interface ImagesState {
  /** data: URLs (b64 inflated) ready to render. */
  images: string[];
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

interface EmbeddingsState {
  embedding: number[] | null;
  dims: number;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const chatInitial: ChatState = {
  output: '',
  reasoning: '',
  loading: false,
  error: null,
  lastUpdated: null,
};

const imagesInitial: ImagesState = {
  images: [],
  loading: false,
  error: null,
  lastUpdated: null,
};

const embeddingsInitial: EmbeddingsState = {
  embedding: null,
  dims: 0,
  loading: false,
  error: null,
  lastUpdated: null,
};

const _chat$ = new BehaviorSubject<ChatState>(chatInitial);
const _images$ = new BehaviorSubject<ImagesState>(imagesInitial);
const _embeddings$ = new BehaviorSubject<EmbeddingsState>(embeddingsInitial);

interface DeltaWithReasoning {
  content?: string | null;
  reasoning?: string | null;
  reasoning_content?: string | null;
}

interface MessageWithReasoning {
  content?: string | null;
  reasoning?: string | null;
  reasoning_content?: string | null;
}

export const byocService = {
  chat$: _chat$.asObservable() as Observable<ChatState>,
  images$: _images$.asObservable() as Observable<ImagesState>,
  embeddings$: _embeddings$.asObservable() as Observable<EmbeddingsState>,
  get chat(): ChatState { return _chat$.getValue(); },
  get images(): ImagesState { return _images$.getValue(); },
  get embeddings(): EmbeddingsState { return _embeddings$.getValue(); },

  async runChat(payload: ByocChatPayload, opts: { stream: boolean }): Promise<void> {
    _chat$.next({ ...chatInitial, loading: true });
    try {
      let output = '';
      let reasoning = '';
      if (opts.stream) {
        const stream = await openaiSdk.chatStream(payload);
        for await (const chunk of stream) {
          const delta = chunk.choices[0]?.delta as DeltaWithReasoning | undefined;
          if (!delta) continue;
          if (delta.content) output += stripHeader(delta.content);
          if (delta.reasoning) reasoning += stripHeader(delta.reasoning);
          if (delta.reasoning_content) reasoning += stripHeader(delta.reasoning_content);
          _chat$.next({
            ..._chat$.getValue(),
            output,
            reasoning,
            loading: true,
          });
        }
      } else {
        const res = await openaiSdk.chat(payload);
        const msg = res.choices[0]?.message as MessageWithReasoning | undefined;
        output = stripHeader(msg?.content);
        reasoning = stripHeader(msg?.reasoning) + stripHeader(msg?.reasoning_content);
      }
      _chat$.next({
        output,
        reasoning,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _chat$.next({ ..._chat$.getValue(), loading: false, error: byocErrorMessage(err) });
    }
  },

  async runImages(payload: {
    model: string;
    prompt: string;
    size: '1024x1024' | '1024x1792' | '1792x1024';
    n: number;
  }): Promise<void> {
    _images$.next({ ...imagesInitial, loading: true });
    try {
      const res = await openaiSdk.images(payload);
      const data = res.data ?? [];
      const images = data
        .map((d) => {
          if (d.b64_json) return `data:image/png;base64,${d.b64_json}`;
          return d.url ?? '';
        })
        .filter(Boolean);
      _images$.next({
        images,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _images$.next({ ..._images$.getValue(), loading: false, error: byocErrorMessage(err) });
    }
  },

  async runEmbeddings(payload: { model: string; input: string }): Promise<void> {
    _embeddings$.next({ ...embeddingsInitial, loading: true });
    try {
      const res = await openaiSdk.embeddings(payload);
      const embedding =
        (res.data?.[0]?.embedding as number[] | undefined) ??
        ((res as unknown as { embedding?: number[] }).embedding ?? null);
      _embeddings$.next({
        embedding,
        dims: embedding?.length ?? 0,
        loading: false,
        error: null,
        lastUpdated: new Date().toISOString(),
      });
    } catch (err) {
      _embeddings$.next({
        ..._embeddings$.getValue(),
        loading: false,
        error: byocErrorMessage(err),
      });
    }
  },

  reset(): void {
    _chat$.next(chatInitial);
    _images$.next(imagesInitial);
    _embeddings$.next(embeddingsInitial);
  },
};
