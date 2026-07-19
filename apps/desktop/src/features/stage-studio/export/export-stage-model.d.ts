import type { ExportRenderInputReference } from "@narracut/contracts";
export function resolveApprovedRenderCandidate(input: unknown): { valid: true; value: ExportRenderInputReference } | { valid: false; error: string };
export function safeExportName(projectName: string): string;
