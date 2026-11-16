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
export function selectedBrowserFiles(paths: string[]): File[] {
  if (paths.length > 16) throw new Error('Message size limit');
  return paths.map(path => {
    const file = files.get(path);
    if (!file) throw new Error('Please select the file again');
    if (file.size > 2 * 1024 * 1024 * 1024) throw new Error('File size limit is 2 GiB');
    return file;
  });
}
