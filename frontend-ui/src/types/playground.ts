export type Modality =
  | 'llm'
  | 'text-to-image'
  | 'image-to-image'
  | 'image-to-video'
  | 'image-to-text'
  | 'audio-to-text'
  | 'text-to-speech'
  | 'upscale'
  | 'segment-anything-2';

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant';
  content: string;
}

export interface LlmPayload {
  model: string;
  messages: ChatMessage[];
  max_tokens: number;
  stream: boolean;
  temperature?: number;
}

export interface ChatChoiceMessage {
  content?: string;
  reasoning?: string;
  reasoning_content?: string;
}

export interface ChatChoice {
  delta?: ChatChoiceMessage;
  message?: ChatChoiceMessage;
}

export interface ChatCompletionResponse {
  choices: ChatChoice[];
}

export interface TextToImagePayload {
  prompt: string;
  model_id: string;
  width: number;
  height: number;
  num_images_per_prompt: number;
  num_inference_steps: number;
  guidance_scale: number;
  safety_check?: boolean;
  negative_prompt?: string;
  seed?: number;
}

export interface MediaItem {
  url: string;
  seed?: number;
  nsfw?: boolean;
}

export interface ImagesResponse {
  images: MediaItem[];
}

export interface TextResponse {
  text: string;
}

export interface AudioResponse {
  audio: MediaItem;
}

export interface CapabilitiesModel {
  name: string;
  status: { Cold: number; Warm: number };
}

export interface CapabilitiesPipeline {
  type: string;
  models: CapabilitiesModel[];
}

export interface CapabilitiesOrchestrator {
  address: string;
  pipelines: CapabilitiesPipeline[];
}

export interface NetworkCapabilities {
  orchestrators: CapabilitiesOrchestrator[];
}

/** What the gateway endpoint actually returns — we normalize before storing. */
export interface RawCapabilitiesResponse {
  orchestrators?: Record<string, RawCapabilityPipelineMap> | unknown;
}

export type RawCapabilityPipelineMap = Record<string, RawCapabilityModelMap>;
export type RawCapabilityModelMap = Record<string, { Cold: number; Warm: number }>;

export interface HistoryEntry {
  id: string;
  modality: Modality;
  timestamp: string;
  modelId?: string;
  prompt?: string;
  summary: string;
  output?: HistoryOutput;
}

export type HistoryOutput =
  | { kind: 'text'; text: string }
  | { kind: 'images'; images: MediaItem[] }
  | { kind: 'audio'; audio: MediaItem };
