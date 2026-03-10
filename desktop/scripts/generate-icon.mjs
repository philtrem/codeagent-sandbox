import sharp from 'sharp';
import { readFileSync } from 'fs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const iconsDir = resolve(__dirname, '..', 'src-tauri', 'icons');
const svgPath = resolve(iconsDir, 'icon.svg');
const pngPath = resolve(iconsDir, 'icon-1024.png');

const svg = readFileSync(svgPath);

await sharp(svg, { density: 300 })
  .resize(1024, 1024)
  .png()
  .toFile(pngPath);

console.log(`Generated ${pngPath}`);
