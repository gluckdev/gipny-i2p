// Browser files stay in memory until sent; these handles are never native paths.
const files = new Map<string, File>();
let nextId = 0;
export function registerBrowserFile(file: File): string {
  const key = `browser-file:${++nextId}`;
  files.set(key, file);
  return key;
}
export function forgetBrowserFiles(paths: string[]): void { paths.forEach((path) => files.delete(path)); }
export function clearBrowserFiles(): void { files.clear(); }
export async function browserAttachments(paths: string[]): Promise<{ name: string; data: string }[]> {
  const result = [];
  for (const path of paths) {
    const file = files.get(path);
    if (!file) throw new Error('Please select the file again');
    const bytes = new Uint8Array(await file.arrayBuffer());
    let binary = '';
    for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
    result.push({ name: file.name, data: btoa(binary) });
  }
  paths.forEach((path) => files.delete(path));
  return result;
}
export function downloadBrowserFile(base64: string, name: string): void {
  const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
  const url = URL.createObjectURL(new Blob([bytes], { type: 'application/octet-stream' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
