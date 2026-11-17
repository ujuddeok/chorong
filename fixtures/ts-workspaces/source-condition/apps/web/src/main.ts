import { Client } from '@fx/client';
import { openSession, type Session } from '@fx/core';
import { button } from '@fx/ui/components/button';

export async function main(): Promise<string> {
  const s: Session = openSession('a');
  const c = new Client(s.id);
  return button(await c.call('/x')) + s.options.timeoutMs;
}
