export class Client {
  constructor(readonly url: string) {}

  fetch(path: string): string {
    return this.url + path;
  }
}
