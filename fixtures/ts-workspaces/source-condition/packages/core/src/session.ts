export interface Session {
  id: string;
  options: { timeoutMs: number };
}

export function openSession(id: string): Session {
  return { id, options: { timeoutMs: 1000 } };
}
