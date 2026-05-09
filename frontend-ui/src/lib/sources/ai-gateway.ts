import { configService } from '../../services/config.service.js';
import type {
  AudioResponse,
  ChatChoiceMessage,
  ChatCompletionResponse,
  ImagesResponse,
  LlmPayload,
  NetworkCapabilities,
  RawCapabilitiesResponse,
  TextResponse,
  TextToImagePayload,
} from '../../types/playground.js';

function gatewayUrl(): string {
  return configService.value.gatewayUrl.replace(/\/$/, '');
}

function bearerHeaders(): Record<string, string> {
  const token = configService.value.gatewayBearer;
  return token ? { Authorization: `Bearer ${token}` } : {};
}

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${gatewayUrl()}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json', ...bearerHeaders() },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`HTTP ${res.status} ${path}${text ? `: ${text.slice(0, 200)}` : ''}`);
  }
  return res.json() as Promise<T>;
}

async function postMultipart<T>(path: string, fields: Record<string, string | number | boolean | File | undefined>): Promise<T> {
  const fd = new FormData();
  for (const [k, v] of Object.entries(fields)) {
    if (v === undefined) continue;
    if (v instanceof File) fd.append(k, v, v.name);
    else fd.append(k, String(v));
  }
  const res = await fetch(`${gatewayUrl()}${path}`, {
    method: 'POST',
    headers: { Accept: 'application/json', ...bearerHeaders() },
    body: fd,
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`HTTP ${res.status} ${path}${text ? `: ${text.slice(0, 200)}` : ''}`);
  }
  return res.json() as Promise<T>;
}

async function fetchJson<T>(path: string): Promise<T> {
  const res = await fetch(`${gatewayUrl()}${path}`, {
    headers: { Accept: 'application/json', ...bearerHeaders() },
  });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${path}`);
  return res.json() as Promise<T>;
}

/**
 * POSTs to /llm with `stream: true` and yields incremental chat-completion deltas
 * parsed from the Server-Sent Events stream.
 */
export async function* streamLlm(
  payload: Omit<LlmPayload, 'stream'>,
): AsyncGenerator<ChatChoiceMessage, void, void> {
  const res = await fetch(`${gatewayUrl()}/llm`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'text/event-stream',
      ...bearerHeaders(),
    },
    body: JSON.stringify({ ...payload, stream: true }),
  });
  if (!res.ok || !res.body) {
    const text = await res.text().catch(() => '');
    throw new Error(`HTTP ${res.status} /llm${text ? `: ${text.slice(0, 200)}` : ''}`);
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const events = buffer.split('\n\n');
    buffer = events.pop() ?? '';
    for (const evt of events) {
      const dataLines = evt
        .split('\n')
        .filter((l) => l.startsWith('data:'))
        .map((l) => l.slice(5).trim());
      if (dataLines.length === 0) continue;
      const dataStr = dataLines.join('\n');
      if (dataStr === '[DONE]') return;
      try {
        const parsed = JSON.parse(dataStr) as ChatCompletionResponse;
        const delta = parsed.choices?.[0]?.delta;
        if (delta) yield delta;
      } catch {
        // ignore non-JSON keep-alives
      }
    }
  }
}

/**
 * Normalize the gateway's getNetworkCapabilities response into the
 * `{ orchestrators: [{ address, pipelines: [{ type, models: [{ name, status }] }] }] }`
 * shape the playground views expect.
 *
 * Mirrors the logic in `livepeer-tools-ui`'s `transformNetworkCapabilitiesToAICapabilities`.
 * Models are aggregated from three sources inside each orchestrator entry:
 *
 *   1) `orch.hardware[]`         → { pipeline, model_id }
 *   2) `orch.capabilities_prices[]` → { capability (id), pipeline?, constraint } —
 *      where `capability` resolves through the top-level `capabilities_names`
 *      table; for BYOC (capability id 37 / "byoc") the `constraint` is itself
 *      the pipeline name and models come from `capability_options`.
 *   3) `orch.capability_options` → { [pipeline]: [{ model, ... }] }
 *
 * Pipeline names are title-cased ("llm" → "Llm", "text-to-image" → "Text-to-image")
 * so existing `modelsForPipeline(data, 'Llm')` calls keep matching.
 */
function normalizeCapabilities(raw: RawCapabilitiesResponse | unknown): NetworkCapabilities {
  if (!raw || typeof raw !== 'object') return { orchestrators: [] };
  const root = raw as {
    orchestrators?: unknown;
    capabilities_names?: Record<string, string>;
  };
  const orchsRaw = root.orchestrators;
  if (!Array.isArray(orchsRaw)) return { orchestrators: [] };
  const namesById = root.capabilities_names ?? {};

  interface PipelineAcc {
    type: string;
    models: Map<string, { name: string; status: { Cold: number; Warm: number } }>;
  }
  type OrchAcc = { address: string; pipelines: Map<string, PipelineAcc> };

  // Canonicalize pipeline names so the various source shapes don't produce
  // duplicate entries: lowercase, replace runs of whitespace with hyphens,
  // then capitalize the first letter for display ("text to image" and
  // "text-to-image" both become "Text-to-image").
  const titleCase = (s: string): string => {
    if (!s) return '';
    const kebab = s.trim().toLowerCase().replace(/\s+/g, '-');
    return kebab.charAt(0).toUpperCase() + kebab.slice(1);
  };

  const addPipelineModel = (orch: OrchAcc, pipelineName: string, modelName: string): void => {
    if (!pipelineName || !modelName) return;
    const type = titleCase(pipelineName);
    let p = orch.pipelines.get(type);
    if (!p) {
      p = { type, models: new Map() };
      orch.pipelines.set(type, p);
    }
    if (!p.models.has(modelName)) {
      p.models.set(modelName, { name: modelName, status: { Cold: 0, Warm: 1 } });
    }
  };

  const ensurePipeline = (orch: OrchAcc, pipelineName: string): void => {
    if (!pipelineName) return;
    const type = titleCase(pipelineName);
    if (!orch.pipelines.has(type)) {
      orch.pipelines.set(type, { type, models: new Map() });
    }
  };

  const map = new Map<string, OrchAcc>();

  for (const orch of orchsRaw as Array<{
    address?: string;
    hardware?: Array<{ pipeline?: string; model_id?: string }> | null;
    capabilities_prices?: Array<{ capability?: number | string; pipeline?: string; constraint?: string }> | null;
    capability_options?: Record<string, Array<{ model?: string }>>;
  }>) {
    if (!orch || typeof orch !== 'object' || !orch.address) continue;
    const address = orch.address.toLowerCase();
    let acc = map.get(address);
    if (!acc) {
      acc = { address, pipelines: new Map() };
      map.set(address, acc);
    }

    if (Array.isArray(orch.hardware)) {
      for (const hw of orch.hardware) {
        addPipelineModel(acc, String(hw?.pipeline ?? ''), String(hw?.model_id ?? ''));
      }
    }

    if (Array.isArray(orch.capabilities_prices)) {
      for (const price of orch.capabilities_prices) {
        const capId = String(price?.capability ?? '');
        const capName = namesById[capId];
        const isByoc = capId === '37' || (capName ?? '').toLowerCase() === 'byoc';
        if (isByoc) {
          // For BYOC, constraint is the pipeline name (e.g. "openai-chat-completions").
          // Models come from capability_options below, not here.
          ensurePipeline(acc, String(price?.constraint ?? ''));
        } else {
          const pipelineName = (price?.pipeline ?? capName ?? '').toLowerCase();
          addPipelineModel(acc, pipelineName, String(price?.constraint ?? ''));
        }
      }
    }

    if (orch.capability_options && typeof orch.capability_options === 'object') {
      for (const [pipelineName, options] of Object.entries(orch.capability_options)) {
        if (!Array.isArray(options)) continue;
        for (const opt of options) {
          if (opt?.model) addPipelineModel(acc, pipelineName, opt.model);
        }
      }
    }
  }

  const orchestrators: NetworkCapabilities['orchestrators'] = [];
  for (const acc of map.values()) {
    const pipelines = [...acc.pipelines.values()].map((p) => ({
      type: p.type,
      models: [...p.models.values()],
    }));
    if (pipelines.length === 0) continue; // drop orchs without any AI capabilities
    orchestrators.push({ address: acc.address, pipelines });
  }
  return { orchestrators };
}

export const aiGateway = {
  llm(payload: Omit<LlmPayload, 'stream'>) {
    return postJson<ChatCompletionResponse>('/llm', { ...payload, stream: false });
  },
  streamLlm,
  textToImage(payload: TextToImagePayload) {
    return postJson<ImagesResponse>('/text-to-image', payload);
  },
  imageToImage(fields: {
    image: File;
    model_id: string;
    prompt: string;
    negative_prompt?: string;
    strength?: number;
    guidance_scale?: number;
    num_inference_steps?: number;
    num_images_per_prompt?: number;
    safety_check?: boolean;
    seed?: number;
  }) {
    return postMultipart<ImagesResponse>('/image-to-image', fields);
  },
  imageToVideo(fields: {
    image: File;
    model_id: string;
    width: number;
    height: number;
    fps: number;
    motion_bucket_id: number;
    noise_aug_strength: number;
    seed?: number;
  }) {
    return postMultipart<ImagesResponse>('/image-to-video', fields);
  },
  imageToText(fields: { image: File; model_id: string; prompt?: string }) {
    return postMultipart<TextResponse>('/image-to-text', fields);
  },
  audioToText(fields: { audio: File; model_id: string }) {
    return postMultipart<TextResponse>('/audio-to-text', fields);
  },
  textToSpeech(fields: { prompt: string; model_id: string }) {
    return postMultipart<AudioResponse>('/text-to-speech', fields);
  },
  upscale(fields: { image: File; model_id: string; prompt?: string; safety_check?: boolean; seed?: number }) {
    return postMultipart<ImagesResponse>('/upscale', { prompt: 'not needed', ...fields });
  },
  segmentAnything2(fields: { image: File; model_id: string; point_coords?: string; point_labels?: string }) {
    return postMultipart<ImagesResponse>('/segment-anything-2', fields);
  },
  async networkCapabilities(): Promise<NetworkCapabilities> {
    const raw = await fetchJson<RawCapabilitiesResponse>('/getNetworkCapabilities');
    return normalizeCapabilities(raw);
  },
};

// re-exported for unit tests
export const _internals = { normalizeCapabilities };

/** Resolve the final URL for a media item that may use a relative path. */
export function resolveMediaUrl(url: string | undefined | null): string {
  if (!url) return '';
  if (/^https?:\/\//i.test(url) || url.startsWith('data:')) return url;
  return `${gatewayUrl()}${url.startsWith('/') ? '' : '/'}${url}`;
}
