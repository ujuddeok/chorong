export async function retry<T>(attempts: number, run: () => Promise<T>): Promise<T> {
  for (let i = 1; ; i++) {
    try {
      return await run();
    } catch (e) {
      if (i >= attempts) throw e;
    }
  }
}
