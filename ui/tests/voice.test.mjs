// Run with the same Chromium selection as the layout suite.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright';
import { createServer } from 'vite';
const server = await createServer({ logLevel: 'error', server: { host: '127.0.0.1', port: 0, fs: { allow: [process.cwd(), new URL('../../browser-router/pkg', import.meta.url).pathname] } } });
await server.listen();
const base = 'http://127.0.0.1:' + server.httpServer.address().port;
const browser = await chromium.launch({ executablePath: process.env.GIPNY_CHROMIUM || undefined,
  args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required'] });
try {
  const context = await browser.newContext({ viewport: { width: 360, height: 740 }, permissions: ['microphone'] });
  const pages = await Promise.all([context.newPage(), context.newPage()]);
  const capture = async (page, name) => {
    if (!process.env.GIPNY_LAYOUT_ARTIFACTS) return;
    await mkdir(process.env.GIPNY_LAYOUT_ARTIFACTS, { recursive: true });
    await page.screenshot({ path: process.env.GIPNY_LAYOUT_ARTIFACTS + '/' + name + '.png' });
  };
  const errors = [];
  let firstEcho;
  let retriedProbe = false;
  for (const page of pages) page.on('pageerror', e => errors.push(String(e)));
  for (const [side, page] of pages.entries()) {
    await page.goto(base + '/dev/frame.html?scene=chat&lang=en&theme=' + (side ? 'dark' : 'light'));
    await page.waitForSelector('.voice-call');
    await page.evaluate(async () => {
      const { emit } = await import('/node_modules/.vite/deps/@tauri-apps_api_event.js');
      const { Api } = await import('/src/app/api.ts');
      window.receiveCall = event => emit('core_event', { CallFrame: { ...event, contact_id: 2 } });
      Api.sendMessage = async (_peer, _body, attachments) => { window.lastClip = attachments[0]; await window.voiceSendClip(attachments); return 1; };
      window.sentKinds = [];
      Api.sendCallFrame = (_peer, frame, lane) => {
        window.sentKinds.push(JSON.parse(frame).kind);
        return window.voiceSend(frame, lane);
      };
      const get = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      window.tracks = [];
      navigator.mediaDevices.getUserMedia = async constraints => {
        const stream = await get(constraints); window.tracks.push(...stream.getTracks()); return stream;
      };
    });
    await page.exposeFunction('voiceSendClip', attachments => pages[0].evaluate(data => window.workerCall(data), { side, command: 'sendClip', attachments }));
    await page.exposeFunction('voiceSend', async (frame, lane) => {
      const { kind, seq } = JSON.parse(frame);
      // Each control leg can be fresh when the complete exchange exceeds 30 s.
      if (kind === 'invite' || kind === 'accept') await new Promise(resolve => setTimeout(resolve, 16_000));
      // Deliver the first echo only after the caller retries its single-lane
      // probe. Lose the next two probes so only the delayed original can
      // finish this single-lane measurement.
      if (side === 1 && kind === 'echo' && seq === 1) {
        await new Promise(resolve => { firstEcho = resolve; });
      }
      if (side === 0 && kind === 'probe' && seq === 2) {
        assert.ok(firstEcho, 'the first probe reached the peer');
        retriedProbe = true;
        firstEcho();
        return;
      }
      if (side === 0 && kind === 'probe' && seq === 3) return;
      return pages[0].evaluate(data => window.workerCall(data), { side, command: 'send', frame, lane });
    });
  }
  await pages[0].exposeFunction('deliverToPage', (side, event) => pages[side].evaluate(e => window.receiveCall(e), event));
  await pages[0].evaluate(async () => {
    const worker = new Worker('/tests/voice-worker.js', { type: 'module' });
    const waiting = new Map();
    let next = 0;
    worker.onmessage = ({ data }) => {
      if (data.event) { void window.deliverToPage(data.side, data.event); return; }
      const pair = waiting.get(data.key);
      waiting.delete(data.key);
      if (data.error) pair.reject(Error(data.error)); else pair.resolve(data.value);
    };
    worker.onerror = e => { for (const pair of waiting.values()) pair.reject(Error(e.message)); };
    window.workerCall = data => new Promise((resolve, reject) => {
      const key = ++next; waiting.set(key, { resolve, reject }); worker.postMessage({ ...data, key });
    });
    await window.workerCall({ command: 'init' });
  });
  await pages[0].locator('.voice-call').click();
  await pages[1].getByRole('button', { name: 'Answer', exact: true }).waitFor();
  assert.equal(await pages[1].evaluate(() => window.tracks.length), 0);
  await capture(pages[1], 'incoming-call');
  await pages[1].getByRole('button', { name: 'Answer', exact: true }).click();
  await pages[1].waitForFunction(() => window.sentKinds.includes('probe'));
  for (const page of pages) {
    await page.waitForFunction(() => window.sentKinds.includes('audio') && window.sentKinds.includes('ack'));
    const mute = page.getByRole('button', { name: 'Mute', exact: true });
    await capture(page, 'active-call-' + pages.indexOf(page));
    await mute.click();
    assert.equal(await page.evaluate(() => window.tracks.at(-1).enabled), false);
    await page.getByRole('button', { name: 'Unmute', exact: true }).click();
    const bounds = await page.locator('.voice-panel').boundingBox();
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 361);
  }
  assert.ok(retriedProbe);
  await pages[1].getByRole('button', { name: 'End call' }).click();
  for (const page of pages) {
    await page.waitForSelector('.voice-panel', { state: 'detached' });
    assert.ok(await page.evaluate(() => window.tracks.every(t => t.readyState === 'ended')));
  }
  const report = await pages[0].evaluate(() => window.workerCall({ command: 'report' }));
  assert.ok(report.ciphertexts >= 12);
  assert.deepEqual(report.messages, [0, 0]);
  assert.deepEqual(report.outbox, [0, 0]);
  const page = pages[0];
  await page.locator('.voice-clip').click();
  await page.getByText('Recording', { exact: true }).waitFor();
  await capture(page, 'recording');
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  assert.equal(await page.evaluate(() => window.tracks.at(-1).enabled), false);
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.voice-panel audio')?.readyState >= 1);
  await capture(page, 'preview');
  const wave = await page.evaluate(async () => {
    const audio = document.querySelector('.voice-panel audio');
    const bytes = new Uint8Array(await (await fetch(audio.src)).arrayBuffer());
    return { riff: String.fromCharCode(...bytes.slice(0, 4)), size: bytes.length,
      duration: audio.duration, stopped: window.tracks.every(t => t.readyState === 'ended') };
  });
  assert.equal(wave.riff, 'RIFF'); assert.ok(wave.size > 44 && wave.duration >= .25 && wave.stopped);
  await page.getByRole('button', { name: 'Send', exact: true }).last().click();
  await page.waitForSelector('.voice-panel', { state: 'detached' });
  await page.waitForFunction(async () => {
    const clip = await window.workerCall({ side: 1, command: 'receivedClip' });
    return clip && clip.name === window.lastClip.name && clip.data === window.lastClip.data;
  });
  await page.locator('.voice-clip').click();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.waitForFunction(() => window.tracks.every(track => track.readyState === 'ended'));
  assert.deepEqual(errors, []);
  console.log('PASS · two encrypted browser profiles, call preflight, bidirectional audio/ACK, consent, mute, hangup, WAV preview, exact received bytes and narrow dialogs', report);
} finally { await browser.close(); await server.close(); }
