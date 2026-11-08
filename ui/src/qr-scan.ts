import { errorText } from './error-text';
import { t as tr } from './i18n';
import jsQR from 'jsqr';
import type { QrPass, QrReply } from './qr-worker';
import { h } from './view';

/** Longest edge of a live frame handed to the decoder.
 *
 * A gipny card is a dense code: 500+ base64 characters puts it around QR
 * version 18, which is 177 modules across. The old cap of 640 left under four
 * pixels per module — below what jsQR can sample — so the scan never fired no
 * matter how squarely the code was framed. 1280 gives roughly seven when the
 * code fills the long edge; it rarely does, see `CENTRE`. */
const LIVE_MAX = 1280;

/** A still, and every so often a live frame too, get a second and finer try.
 * A phone held close fills far more of the sensor than the frame we downscale
 * to, and those extra modules are exactly what a dense code needs. */
const FINE_MAX = 2048;

/** The share of the frame's short edge that the centre pass keeps.
 *
 * A phone is held upright, so the code sits inside the short edge of a tall
 * frame, and the guide square on screen puts it in the middle. On a 1080×1920
 * stream a code filling 60% of the width is 650 px across: 3.7 px per module
 * at full resolution and 2.4 after the live downscale, which is why an Honor
 * showed the picture and never read it. Decoding only the centre square at
 * its own resolution keeps every pixel the code has while staying cheap. */
const CENTRE = 0.8;

/** The sensor resolution asked for. `ideal`, so a camera that cannot give it
 * hands over its nearest size instead of refusing; the frame loop downscales
 * for the live pass anyway, and the centre and fine passes are where the
 * extra pixels turn into modules jsQR can sample. */
const WANT_WIDTH = 2560;
const WANT_HEIGHT = 1920;

/** Decoding costs a few milliseconds, and doing it on every animation frame
 * only heats the phone up. Roughly eleven attempts a second is far more than
 * anyone needs to point a camera at a code. */
const EVERY_MS = 90;

/** How often the live path spends a pass on the full sensor resolution. */
const FINE_EVERY = 4;

/** The worker gets this long per frame; three passes take about two seconds
 * on a slow phone, so a frame still unanswered here means a worker that is
 * not going to answer. */
const FRAME_DEADLINE_MS = 10_000;

/** One canvas for every decode, reused: a fresh element per frame is a
 * per-frame allocation the GC gets to hear about. */
const work = document.createElement('canvas');

/** Decode `source` scaled to fit `max` on its long edge, and return the text.
 *
 * `invert` lets jsQR also look for a code reversed out of a light background;
 * it doubles the work, so the live loop only pays it on its finer passes. */
function decode(
  source: CanvasImageSource,
  w: number,
  hgt: number,
  max: number,
  invert: boolean,
  crop?: { x: number; y: number; w: number; h: number },
): string | null {
  const sw = crop ? crop.w : w;
  const sh = crop ? crop.h : hgt;
  const scale = Math.min(1, max / Math.max(sw, sh));
  const width = Math.max(1, Math.round(sw * scale));
  const height = Math.max(1, Math.round(sh * scale));
  work.width = width;
  work.height = height;
  const ctx = work.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  if (crop) ctx.drawImage(source, crop.x, crop.y, crop.w, crop.h, 0, 0, width, height);
  else ctx.drawImage(source, 0, 0, width, height);
  const img = ctx.getImageData(0, 0, width, height);
  const found = jsQR(img.data, width, height, {
    inversionAttempts: invert ? 'attemptBoth' : 'dontInvert',
  });
  return found?.data?.trim() || null;
}

/** Read a QR code with the device camera.
 *
 * The webview asks for the camera itself (wry's `RustWebChromeClient` turns
 * that into the Android runtime prompt), so there is nothing to request from
 * Rust — but the permission can still be refused, and a desktop webview may
 * have no camera at all. Both end as a message, never as a dead screen, and
 * `scanQrInFile` is there for exactly those cases.
 *
 * Decoding is done here rather than with `BarcodeDetector`: that API is absent
 * from some Android WebViews, and a silent "scanning forever" is worse than one
 * small dependency. */
export class QrScanner {
  el: HTMLElement;
  private video: HTMLVideoElement;
  private stream: MediaStream | null = null;
  private frame: number | null = null;
  private lastTry = 0;
  private tries = 0;
  private stopped = false;
  /** The decoder thread, when the engine can run one (see `start`). */
  private worker: Worker | null = null;
  /** A frame is with the worker; the loop skips frames until it answers. */
  private decoding = false;
  /** A frame the worker never answers for ends the worker, not the scan. */
  private frameTimer: number | null = null;

  constructor(private onResult: (text: string) => void, private onError: (message: string) => void) {
    this.video = h('video', { class: 'qr-video', playsinline: 'true', muted: 'true' }) as HTMLVideoElement;
    this.video.muted = true;
    this.el = h('div', { class: 'qr-scan' },
      this.video,
      h('div', { class: 'qr-frame' }),
      h('div', { class: 'qr-hint' }, tr('qr_scan.point_the_camera_at_your_contact_s_qr')),
    );
  }

  async start(): Promise<void> {
    if (!navigator.mediaDevices?.getUserMedia) {
      this.onError(tr('qr_scan.camera_is_unavailable_in_this_window'));
      return;
    }
    // `focusMode` is Chromium's continuous autofocus and is not in the DOM
    // types, but the spec has browsers drop constraints they do not implement,
    // so passing it costs nothing where it is unknown. It is what makes a dense
    // code readable at all: a camera left on one-shot focus blurs exactly the
    // small print that carries the modules.
    const video: MediaTrackConstraints & { focusMode?: string } = {
      facingMode: { ideal: 'environment' },
      width: { ideal: WANT_WIDTH },
      height: { ideal: WANT_HEIGHT },
      focusMode: 'continuous',
    };
    try {
      // The back camera is the one people point at things; `ideal` throughout
      // rather than `exact`, so a laptop with one camera — or a fixed-focus
      // webcam — still starts.
      this.stream = await navigator.mediaDevices.getUserMedia({ video, audio: false });
    } catch (e) {
      const name = (e as { name?: string }).name ?? '';
      this.onError(name === 'NotAllowedError'
        ? tr('qr_scan.camera_permission_denied')
        : name === 'NotFoundError' ? tr('qr_scan.camera_not_found') : tr('qr_scan.camera', { p0: (errorText(e)) }));
      return;
    }
    this.video.srcObject = this.stream;
    try {
      await this.video.play();
    } catch { /* autoplay policies; the frame loop still reads it */ }
    // Decoding takes a few hundred milliseconds a pass on a phone, so it runs
    // on a worker when the engine can hand it frames (ImageBitmap +
    // OffscreenCanvas, Chromium 69+); the main thread keeps the preview and
    // the taps smooth. Without them, or if the worker fails to come up, the
    // same passes run here, as before.
    if (typeof OffscreenCanvas !== 'undefined' && typeof Worker !== 'undefined' && typeof createImageBitmap === 'function') {
      try {
        this.worker = new Worker(new URL('./qr-worker.ts', import.meta.url), { type: 'module' });
        this.worker.onmessage = (event: MessageEvent<QrReply>) => {
          this.clearFrameTimer();
          this.decoding = false;
          if (this.stopped) return;
          if (event.data.text) { this.stop(); this.onResult(event.data.text); }
        };
        this.worker.onerror = () => this.dropWorker();
      } catch { this.worker = null; }
    }
    this.frame = requestAnimationFrame(this.tick);
  }

  /** The passes of one live round: the whole frame at the working size, the
   * centre square at the sensor's own pixels every other round, and every
   * `FINE_EVERY`-th round the whole frame fine and inverted. */
  private passes(w: number, hgt: number): QrPass[] {
    const passes: QrPass[] = [{ max: LIVE_MAX, invert: false }];
    if (this.tries % 2 === 0) {
      const side = Math.round(Math.min(w, hgt) * CENTRE);
      passes.push({ max: FINE_MAX, invert: false, crop: { x: Math.round((w - side) / 2), y: Math.round((hgt - side) / 2), w: side, h: side } });
    }
    if (this.tries % FINE_EVERY === 0 && Math.max(w, hgt) > LIVE_MAX) passes.push({ max: FINE_MAX, invert: true });
    return passes;
  }

  private tick = (now: number): void => {
    if (this.stopped) return;
    if (now - this.lastTry >= EVERY_MS) {
      this.lastTry = now;
      if (this.worker) {
        this.sendFrame();
      } else {
        const text = this.readFrame();
        if (text) {
          this.stop();
          this.onResult(text);
          return;
        }
      }
    }
    this.frame = requestAnimationFrame(this.tick);
  };

  /** Hand the current frame to the worker, unless it is still on the last one.
   *
   * Anything that keeps the worker from answering — a bitmap this engine will
   * not make from a video, a worker that dies, a reply that never comes —
   * ends the worker, and the next tick decodes on the main thread instead.
   * The scan goes on; only the thread changes. */
  private sendFrame(): void {
    const w = this.video.videoWidth;
    const hgt = this.video.videoHeight;
    if (this.decoding || w <= 0 || hgt <= 0) return;
    this.decoding = true;
    this.tries += 1;
    const passes = this.passes(w, hgt);
    createImageBitmap(this.video).then((bitmap) => {
      if (this.stopped || !this.worker) { bitmap.close(); this.decoding = false; return; }
      this.worker.postMessage({ bitmap, passes }, [bitmap]);
      this.frameTimer = window.setTimeout(() => this.dropWorker(), FRAME_DEADLINE_MS);
    }).catch(() => this.dropWorker());
  }

  /** Give up on the worker; the main-thread passes take over from the next tick. */
  private dropWorker(): void {
    this.clearFrameTimer();
    this.worker?.terminate();
    this.worker = null;
    this.decoding = false;
  }

  private clearFrameTimer(): void {
    if (this.frameTimer != null) { clearTimeout(this.frameTimer); this.frameTimer = null; }
  }

  /** The main-thread fallback: the same passes as the worker's, on this frame. */
  private readFrame(): string | null {
    const w = this.video.videoWidth;
    const hgt = this.video.videoHeight;
    if (w <= 0 || hgt <= 0) return null;
    this.tries += 1;
    for (const pass of this.passes(w, hgt)) {
      const text = decode(this.video, w, hgt, pass.max, pass.invert, pass.crop);
      if (text) return text;
    }
    return null;
  }

  stop(): void {
    this.stopped = true;
    if (this.frame != null) cancelAnimationFrame(this.frame);
    this.frame = null;
    this.clearFrameTimer();
    this.worker?.terminate();
    this.worker = null;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = null;
    this.video.srcObject = null;
  }
}

/** Read a QR out of a picture the person picked: a photo of the other screen,
 * a screenshot, a saved image. The camera can be refused, absent, or simply
 * pointed badly, and this is the way through all three.
 *
 * A still is not a frame. It may be far larger than anything the camera
 * produces, and the code is often a small part of a screenshot — so this walks
 * a few scales, coarsest last, and lets jsQR invert throughout. Several passes
 * over one bitmap cost nothing next to the alternative of a code that simply
 * never reads. */
export function scanQrInFile(
  file: File,
  onResult: (text: string) => void,
  onError: (message: string) => void,
): void {
  void (async () => {
    let source: CanvasImageSource;
    let width: number;
    let height: number;
    try {
      const loaded = await loadBitmap(file);
      source = loaded.source;
      width = loaded.width;
      height = loaded.height;
    } catch {
      onError(tr('qr_scan.could_not_open_the_image_png_jpeg_and'));
      return;
    }
    if (width === 0 || height === 0) {
      onError(tr('qr_scan.the_image_is_empty'));
      return;
    }
    const long = Math.max(width, height);
    for (const max of [long, FINE_MAX, LIVE_MAX, 900, 640]) {
      // Asking to scale to more pixels than the picture has gains nothing and
      // costs a full-size pass.
      if (max > long) continue;
      const text = decode(source, width, height, max, true);
      if (text) {
        onResult(text);
        return;
      }
    }
    onError(tr('qr_scan.no_qr_code_found_in_this_image'));
  })();
}

/** A picture as something drawable, with its EXIF orientation already applied.
 *
 * `createImageBitmap` reads the file without putting an element in the document
 * and can be told to honour the orientation; the `<img>` path is for webviews
 * that predate it. */
async function loadBitmap(
  file: File,
): Promise<{ source: CanvasImageSource; width: number; height: number }> {
  if (typeof createImageBitmap === 'function') {
    try {
      const bmp = await createImageBitmap(file, { imageOrientation: 'from-image' });
      if (bmp.width > 0 && bmp.height > 0) {
        return { source: bmp, width: bmp.width, height: bmp.height };
      }
    } catch { /* not a format it will take; try the DOM path */ }
  }
  const url = URL.createObjectURL(file);
  try {
    const img = new Image();
    await new Promise<void>((resolve, reject) => {
      img.onload = () => resolve();
      img.onerror = () => reject(new Error(tr('qr_scan.could_not_decode_image')));
      img.src = url;
    });
    return { source: img, width: img.naturalWidth, height: img.naturalHeight };
  } finally {
    URL.revokeObjectURL(url);
  }
}
