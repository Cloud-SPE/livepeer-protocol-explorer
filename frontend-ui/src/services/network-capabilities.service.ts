import { BehaviorSubject, type Observable } from 'rxjs';
import { aiGateway } from '../lib/sources/ai-gateway.js';
import type { CapabilitiesModel, NetworkCapabilities } from '../types/playground.js';

interface CapabilitiesState {
  data: NetworkCapabilities | null;
  loading: boolean;
  error: string | null;
  lastUpdated: string | null;
}

const initial: CapabilitiesState = {
  data: null,
  loading: false,
  error: null,
  lastUpdated: null,
};

const _state$ = new BehaviorSubject<CapabilitiesState>(initial);

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export const networkCapabilitiesService = {
  state$: _state$.asObservable() as Observable<CapabilitiesState>,
  get state(): CapabilitiesState { return _state$.getValue(); },

  async load(): Promise<void> {
    if (_state$.getValue().loading) return;
    _state$.next({ ..._state$.getValue(), loading: true, error: null });
    try {
      const data = await aiGateway.networkCapabilities();
      _state$.next({ data, loading: false, error: null, lastUpdated: new Date().toISOString() });
    } catch (err) {
      _state$.next({ ..._state$.getValue(), loading: false, error: errMsg(err) });
    }
  },

  reset(): void { _state$.next(initial); },
};

/**
 * Returns deduplicated model names available for the given pipeline type
 * across all orchestrators. Pipeline names are matched case-insensitively;
 * the normalizer in `ai-gateway.ts` title-cases them ("llm" → "Llm",
 * "text-to-image" → "Text-to-image", "openai-chat-completions" →
 * "Openai-chat-completions"), so callers can pass either form.
 */
export function modelsForPipeline(
  data: NetworkCapabilities | null,
  pipelineType: string,
): CapabilitiesModel[] {
  if (!data) return [];
  const seen = new Map<string, CapabilitiesModel>();
  const target = pipelineType.toLowerCase();
  for (const orch of data.orchestrators) {
    for (const p of orch.pipelines) {
      if (p.type.toLowerCase() !== target) continue;
      for (const m of p.models) {
        if (!seen.has(m.name)) seen.set(m.name, m);
      }
    }
  }
  return [...seen.values()];
}

/** Count of orchestrators advertising the given pipeline in the gateway response. */
export function orchestratorsForPipeline(
  data: NetworkCapabilities | null,
  pipelineType: string,
): number {
  if (!data) return 0;
  const target = pipelineType.toLowerCase();
  let count = 0;
  for (const orch of data.orchestrators) {
    if (orch.pipelines.some((p) => p.type.toLowerCase() === target)) count += 1;
  }
  return count;
}
