/// <reference lib="webworker" />
// The QR decoder off the main thread.
//
// jsQR on a phone takes a few hundred milliseconds per pass (150–900 ms on a
// 2026 budget Honor, measured), and the live loop makes two or three passes a
// round. On the main thread that stalls the camera preview and every tap;
// here it only costs a frame's bitmap. One round per message: the frame as an
// ImageBitmap plus the passes to try, the first decoded text back, or null.
import jsQR from 'jsqr';

export interface QrPass {
  /** Longest edge the source is scaled to fit. */
  max: number;
  /** Also look for a code reversed out of a light background (twice the work). */
  invert: boolean;
  /** Decode only this rectangle of the source, in source pixels. */
  crop?: { x: number; y: number; w: number; h: number };
}

export interface QrJob { bitmap: ImageBitmap; passes: QrPass[]; }
export interface QrReply { text: string | null; }

/** One canvas for every decode, reused: a fresh one per frame is a per-frame
 * allocation the GC gets to hear about. */
let work: OffscreenCanvas | null = null;

export function decodeBitmap(source: ImageBitmap, passes: QrPass[]): string | null {
  for (const pass of passes) {
    const sw = pass.crop ? pass.crop.w : source.width;
    const sh = pass.crop ? pass.crop.h : source.height;
    const scale = Math.min(1, pass.max / Math.max(sw, sh));
    const width = Math.max(1, Math.round(sw * scale));
    const height = Math.max(1, Math.round(sh * scale));
    if (!work) work = new OffscreenCanvas(width, height);
    work.width = width;
    work.height = height;
    const ctx = work.getContext('2d', { willReadFrequently: true });
    if (!ctx) return null;
    if (pass.crop) ctx.drawImage(source, pass.crop.x, pass.crop.y, pass.crop.w, pass.crop.h, 0, 0, width, height);
    else ctx.drawImage(source, 0, 0, width, height);
    const img = ctx.getImageData(0, 0, width, height);
    const found = jsQR(img.data, width, height, { inversionAttempts: pass.invert ? 'attemptBoth' : 'dontInvert' });
    const text = found?.data?.trim();
    if (text) return text;
  }
  return null;
}

self.onmessage = (event: MessageEvent<QrJob>) => {
  const { bitmap, passes } = event.data;
  let text: string | null = null;
  try { text = decodeBitmap(bitmap, passes); } finally { bitmap.close(); }
  const reply: QrReply = { text };
  self.postMessage(reply);
};
