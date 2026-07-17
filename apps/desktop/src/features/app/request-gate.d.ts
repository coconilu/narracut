export interface RequestToken {
  isCurrent(): boolean;
}

export interface RequestGate {
  begin(): RequestToken;
  invalidate(): void;
}

export function createRequestGate(): RequestGate;
