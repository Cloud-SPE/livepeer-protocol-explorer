import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/sources/ai-gateway.js', () => ({
  aiGateway: {
    networkCapabilities: vi.fn(),
  },
}));

const { networkCapabilitiesService, modelsForPipeline } = await import(
  '../../services/network-capabilities.service.js'
);
const { aiGateway } = await import('../../lib/sources/ai-gateway.js');

beforeEach(() => {
  vi.mocked(aiGateway.networkCapabilities).mockReset();
  networkCapabilitiesService.reset();
});

const sample = {
  orchestrators: [
    {
      address: '0xa',
      pipelines: [
        {
          type: 'Llm',
          models: [
            { name: 'llama3', status: { Cold: 0, Warm: 2 } },
            { name: 'mistral', status: { Cold: 1, Warm: 0 } },
          ],
        },
      ],
    },
    {
      address: '0xb',
      pipelines: [
        {
          type: 'Llm',
          models: [{ name: 'llama3', status: { Cold: 0, Warm: 1 } }],
        },
        {
          type: 'Text-to-image',
          models: [{ name: 'sdxl', status: { Cold: 0, Warm: 1 } }],
        },
      ],
    },
  ],
};

describe('networkCapabilitiesService.load', () => {
  it('hydrates state from gateway', async () => {
    vi.mocked(aiGateway.networkCapabilities).mockResolvedValueOnce(sample);
    await networkCapabilitiesService.load();
    expect(networkCapabilitiesService.state.data).toEqual(sample);
    expect(networkCapabilitiesService.state.error).toBeNull();
  });

  it('records errors', async () => {
    vi.mocked(aiGateway.networkCapabilities).mockRejectedValueOnce(new Error('500'));
    await networkCapabilitiesService.load();
    expect(networkCapabilitiesService.state.error).toBe('500');
  });
});

describe('modelsForPipeline', () => {
  it('returns dedup models for a given pipeline type, case-insensitive', () => {
    const out = modelsForPipeline(sample, 'llm');
    expect(out.map((m) => m.name).sort()).toEqual(['llama3', 'mistral']);
  });

  it('returns empty for unknown pipelines', () => {
    expect(modelsForPipeline(sample, 'no-such-type')).toEqual([]);
  });

  it('returns empty for null data', () => {
    expect(modelsForPipeline(null, 'Llm')).toEqual([]);
  });
});
