// tsdown output (JavaScript only; types come from tsc in .types/).
export function openSession(id) { return { id, options: { timeoutMs: 1000 } }; }
export async function retry(attempts, run) { for (let i = 1; ; i++) { try { return await run(); } catch (e) { if (i >= attempts) throw e; } } }
