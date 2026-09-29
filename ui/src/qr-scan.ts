import jsQR from 'jsqr';
import { h } from './view';

/** Longest edge of a live frame handed to the decoder.
 *
 * A gipny card is a dense code: 500+ base64 characters puts it around QR
 * version 18, which is 177 modules across. The old cap of 640 left under four
 * pixels per module — below what jsQR can sample — so the scan never fired no
 * matter how squarely the code was framed. 1280 gives roughly seven. */
const LIVE_MAX = 1280;

/** A still, and every so often a live frame too, get a second and finer try.
 * A phone held close fills far more of the sensor than the frame we downscale
 * to, and those extra modules are exactly what a dense code needs. */
const FINE_MAX = 2048;

/** Decoding costs a few milliseconds, and doing it on every animation frame
 * only heats the phone up. Roughly eleven attempts a second is far more than
 * anyone needs to point a camera at a code. */
const EVERY_MS = 90;

/** How often the live path spends a pass on the full sensor resolution. */
const FINE_EVERY = 4;

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
): string | null {
  const scale = Math.min(1, max / Math.max(w, hgt));
  const width = Math.max(1, Math.round(w * scale));
  const height = Math.max(1, Math.round(hgt * scale));
  work.width = width;
  work.height = height;
  const ctx = work.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(source, 0, 0, width, height);
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

  constructor(private onResult: (text: string) => void, private onError: (message: string) => void) {
    this.video = h('video', { class: 'qr-video', playsinline: 'true', muted: 'true' }) as HTMLVideoElement;
    this.video.muted = true;
    this.el = h('div', { class: 'qr-scan' },
      this.video,
      h('div', { class: 'qr-frame' }),
      h('div', { class: 'qr-hint' }, 'наведите камеру на QR собеседника'),
    );
  }

  async start(): Promise<void> {
    if (!navigator.mediaDevices?.getUserMedia) {
      this.onError('камера недоступна в этом окне');
      return;
    }
    // `focusMode` is Chromium's continuous autofocus and is not in the DOM
    // types, but the spec has browsers drop constraints they do not implement,
    // so passing it costs nothing where it is unknown. It is what makes a dense
    // code readable at all: a camera left on one-shot focus blurs exactly the
    // small print that carries the modules.
    const video: MediaTrackConstraints & { focusMode?: string } = {
      facingMode: { ideal: 'environment' },
      width: { ideal: 1920 },
      height: { ideal: 1080 },
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
        ? 'доступ к камере не разрешён'
        : name === 'NotFoundError' ? 'камера не найдена' : `камера: ${String(e)}`);
      return;
    }
    this.video.srcObject = this.stream;
    try {
      await this.video.play();
    } catch { /* autoplay policies; the frame loop still reads it */ }
    this.frame = requestAnimationFrame(this.tick);
  }

  private tick = (now: number): void => {
    if (this.stopped) return;
    if (now - this.lastTry >= EVERY_MS) {
      this.lastTry = now;
      const text = this.readFrame();
      if (text) {
        this.stop();
        this.onResult(text);
        return;
      }
    }
    this.frame = requestAnimationFrame(this.tick);
  };

  /** One round on the current frame: the working size first, and every
   * `FINE_EVERY`-th round also the sensor's own resolution, since the code may
   * be far larger in frame than the part we keep. */
  private readFrame(): string | null {
    const w = this.video.videoWidth;
    const hgt = this.video.videoHeight;
    if (w <= 0 || hgt <= 0) return null;
    this.tries += 1;
    const text = decode(this.video, w, hgt, LIVE_MAX, false);
    if (text) return text;
    if (this.tries % FINE_EVERY === 0 && Math.max(w, hgt) > LIVE_MAX) {
      return decode(this.video, w, hgt, FINE_MAX, true);
    }
    return null;
  }

  stop(): void {
    this.stopped = true;
    if (this.frame != null) cancelAnimationFrame(this.frame);
    this.frame = null;
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
      onError('изображение не открылось — поддерживаются png, jpeg и webp');
      return;
    }
    if (width === 0 || height === 0) {
      onError('изображение пустое');
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
    onError('в этом изображении не нашлось QR-кода');
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
      img.onerror = () => reject(new Error('decode failed'));
      img.src = url;
    });
    return { source: img, width: img.naturalWidth, height: img.naturalHeight };
  } finally {
    URL.revokeObjectURL(url);
  }
}
