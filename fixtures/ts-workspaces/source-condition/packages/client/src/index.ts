import { openSession, retry, type Session } from '@fx/core';

export class Client {
  readonly session: Session;

  constructor(id: string) {
    this.session = openSession(id);
  }

  call(path: string): Promise<string> {
    return retry(3, async () => `${this.session.id}${path}`);
  }
}
