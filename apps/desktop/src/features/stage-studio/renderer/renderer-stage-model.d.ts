import type { RendererConfig, RendererTimelineInputReference, TimelineDocument } from "@narracut/contracts";

export type RendererCandidateResult =
  | { readonly valid: true; readonly value: RendererTimelineInputReference }
  | { readonly valid: false; readonly error: string };

export function resolveApprovedTimelineCandidate(input: unknown): RendererCandidateResult;
export function defaultRenderConfig(timeline: TimelineDocument | unknown): RendererConfig | null;
