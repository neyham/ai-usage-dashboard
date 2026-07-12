// Generates the app icon set (no native deps) — a dark card with three usage
// bars in the service colors (green / amber / cyan). Run: node scripts/make-icons.mjs
// Produces src-tauri/icons/{32x32,128x128,128x128@2x}.png, icon.ico, icon.icns,
// and app-icon.png (1024, source for `npm run tauri icon` if you'd rather use that).

import zlib from "node:zlib";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.join(here, "..", "src-tauri", "icons");
fs.mkdirSync(outDir, { recursive: true });

const BG = [4, 6, 10, 255];
const BARS = [
  [79, 255, 177, 255], // green  (codex)
  [255, 185, 73, 255], // amber  (claude)
  [89, 205, 255, 255], // cyan   (deepseek)
];
const HEIGHTS = [0.55, 0.82, 0.4];

function genPixels(size) {
  const buf = Buffer.alloc(size * size * 4);
  const margin = Math.floor(size * 0.16);
  const gap = Math.floor(size * 0.06);
  const barW = Math.floor((size - 2 * margin - 2 * gap) / 3);
  const barBot = size - margin;
  const barTop = margin;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let c = BG;
      for (let i = 0; i < 3; i++) {
        const bx = margin + i * (barW + gap);
        const top = barBot - Math.floor((barBot - barTop) * HEIGHTS[i]);
        if (x >= bx && x < bx + barW && y >= top && y < barBot) c = BARS[i];
      }
      const o = (y * size + x) * 4;
      buf[o] = c[0];
      buf[o + 1] = c[1];
      buf[o + 2] = c[2];
      buf[o + 3] = c[3];
    }
  }
  return buf;
}

const crcTable = (() => {
  const t = new Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const t = Buffer.from(type, "ascii");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])), 0);
  return Buffer.concat([len, t, data, crc]);
}
function toPNG(size) {
  const pixels = genPixels(size);
  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    pixels.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  return Buffer.concat([
    sig,
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
function toICO(png) {
  const dir = Buffer.alloc(6);
  dir.writeUInt16LE(1, 2); // type: icon
  dir.writeUInt16LE(1, 4); // count
  const ent = Buffer.alloc(16);
  ent[0] = 0; // width 256 (0 == 256)
  ent[1] = 0; // height 256
  ent.writeUInt16LE(1, 4); // planes
  ent.writeUInt16LE(32, 6); // bpp
  ent.writeUInt32LE(png.length, 8);
  ent.writeUInt32LE(22, 12); // offset = 6 + 16
  return Buffer.concat([dir, ent, png]);
}
function toICNS(png) {
  const type = Buffer.from("ic08", "ascii"); // 256x256 PNG
  const len = Buffer.alloc(4);
  len.writeUInt32BE(png.length + 8, 0);
  const entry = Buffer.concat([type, len, png]);
  const total = Buffer.alloc(4);
  total.writeUInt32BE(8 + entry.length, 0);
  return Buffer.concat([Buffer.from("icns", "ascii"), total, entry]);
}

const png256 = toPNG(256);
const files = {
  "32x32.png": toPNG(32),
  "128x128.png": toPNG(128),
  "128x128@2x.png": png256,
  "app-icon.png": toPNG(1024),
  "icon.ico": toICO(png256),
  "icon.icns": toICNS(png256),
};
for (const [name, data] of Object.entries(files)) {
  fs.writeFileSync(path.join(outDir, name), data);
  console.log(`wrote icons/${name} (${data.length} bytes)`);
}
