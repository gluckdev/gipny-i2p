// The navigation clock includes module transfer, compilation and initialization.
const output = document.querySelector('#result');
const state = globalThis.bootstrapState = { stage: 'module', navigation_to_script_ms: performance.now() };
let heartbeat = performance.now();
let maxHeartbeatGap = 0;
let wasmMax = 0;
let memory;
const interval = setInterval(() => {
  const now = performance.now();
  maxHeartbeatGap = Math.max(maxHeartbeatGap, now - heartbeat);
  heartbeat = now;
  if (memory) wasmMax = Math.max(wasmMax, memory.buffer.byteLength);
}, 100);
try {
  const module = await import('/pkg/gipny_browser_router.js');
  const wasm = await module.default();
  memory = wasm.memory;
  state.module_ready_ms = performance.now();
  state.stage = 'config';
  const response = await fetch('/config.json', { cache: 'no-store', credentials: 'omit', redirect: 'error' });
  if (!response.ok) throw new Error('bootstrap configuration unavailable');
  const config = await response.json();
  state.stage = 'router';
  const started = performance.now();
  const receipt = JSON.parse(await module.bootstrap_probe(
    new URL('/snapshot.tar.gz', location.href).href,
    config.sha256, config.endpoint, config.capability,
  ));
  wasmMax = Math.max(wasmMax, memory.buffer.byteLength);
  const resources = performance.getEntriesByType('resource').map(entry => ({
    path: new URL(entry.name).pathname, duration_ms: entry.duration,
    transfer_bytes: entry.transferSize, decoded_bytes: entry.decodedBodySize,
  }));
  state.result = {
    ...receipt,
    module_ready_ms: state.module_ready_ms,
    navigation_to_ready_ms: started + receipt.ready_after_ms,
    navigation_to_completion_ms: performance.now(),
    wasm_capacity_max_bytes: wasmMax,
    max_heartbeat_gap_ms: maxHeartbeatGap,
    user_agent: navigator.userAgent,
    resources,
  };
  state.stage = 'passed';
  output.textContent = JSON.stringify(state.result, null, 2);
} catch (error) {
  state.error = String(error);
  state.stage = 'failed';
  output.textContent = state.error;
} finally {
  clearInterval(interval);
}
