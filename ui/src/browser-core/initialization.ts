/** WASM runs in the dedicated worker. A Window capability check cannot decide
 * what that realm supports. Keep failure as data until an RPC asks for it so
 * an eager initialization never produces an unhandled rejection. */
export type WasmInitialization = { ok: true; elapsed_ms: number } | { ok: false; error: string };

export async function initializeWorkerWasm(
  initialize: () => Promise<unknown>,
  realm: { WebAssembly?: unknown } = globalThis,
): Promise<WasmInitialization> {
  const started = performance.now();
  try {
    const api = realm.WebAssembly as { instantiate?: unknown } | undefined;
    if (!api || typeof api.instantiate !== 'function') {
      return { ok: false, error: 'WebAssembly is unavailable in the browser worker (WASM_API_UNAVAILABLE).' };
    }
    await initialize();
    return { ok: true, elapsed_ms: Math.round(performance.now() - started) };
  } catch (error) {
    const name = error instanceof Error ? error.name : 'Error';
    const detail = error instanceof Error ? error.message : String(error);
    // CompileError can also mean unsupported instructions or a damaged module.
    // Preserve the actual cause without guessing CSP, Lockdown, or browser settings.
    return { ok: false, error: `WebAssembly initialization failed (WASM_INIT_FAILED, ${name}): ${detail}`.slice(0, 1000) };
  }
}
