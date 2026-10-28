import { copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = dirname(fileURLToPath(import.meta.url));
export const publicFiles = ['index.html', 'styles.css', 'main.js', 'assets/logo.svg'];

export async function build(output) {
  const destination = resolve(output);
  if (destination === root) throw new Error('Build output must differ from the source directory');
  for (const file of publicFiles) {
    const target = resolve(destination, file);
    await mkdir(dirname(target), { recursive: true });
    await copyFile(resolve(root, file), target);
  }
  return destination;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  console.log(`Static site: ${await build(process.argv[2] ?? '_site')}`);
}
