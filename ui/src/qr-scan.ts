import jsQR from 'jsqr';
import { h } from './view';

/** Read a QR code with the device camera.
 *
 * The webview asks for the camera itself (wry's `RustWebChromeClient` turns
 * that into the Android runtime prompt), so there is nothing to request from
 * Rust — but the permission can still be refused, and a desktop webview may
 * have no camera at all. Both end as a message, never as a dead screen.
 *
 * Decoding is done here rather than with `BarcodeDetector`: that API is absent
 * from some Android WebViews, and a silent "scanning forever" is worse than one
 * small dependency. */
export class QrScanner {
  el: HTMLElement;
  private video: HTMLVideoElement;
  private canvas = document.createElement('canvas');
  private stream: MediaStream | null = null;
  private frame: number | null = null;
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
    try {
      // The back camera is the one people point at things; `ideal` rather than
      // `exact` so a laptop with one camera still works.
      this.stream = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: { ideal: 'environment' } },
        audio: false,
      });
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
    this.tick();
  }

  private tick = (): void => {
    if (this.stopped) return;
    const w = this.video.videoWidth;
    const h2 = this.video.videoHeight;
    if (w > 0 && h2 > 0) {
      // Downscale: a card's QR is readable well under 640 px, and a phone
      // decoding full-resolution frames heats up for nothing.
      const scale = Math.min(1, 640 / Math.max(w, h2));
      this.canvas.width = Math.round(w * scale);
      this.canvas.height = Math.round(h2 * scale);
      const ctx = this.canvas.getContext('2d', { willReadFrequently: true });
      if (ctx) {
        ctx.drawImage(this.video, 0, 0, this.canvas.width, this.canvas.height);
        const img = ctx.getImageData(0, 0, this.canvas.width, this.canvas.height);
        const found = jsQR(img.data, img.width, img.height, { inversionAttempts: 'dontInvert' });
        if (found?.data) {
          this.stop();
          this.onResult(found.data.trim());
          return;
        }
      }
    }
    this.frame = requestAnimationFrame(this.tick);
  };

  stop(): void {
    this.stopped = true;
    if (this.frame != null) cancelAnimationFrame(this.frame);
    this.frame = null;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = null;
    this.video.srcObject = null;
  }
}
