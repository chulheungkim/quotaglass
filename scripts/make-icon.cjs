// Generates a 1024x1024 source PNG for `tauri icon` — a purple→blue rounded
// square with a white center mark. Pure Node, no dependencies.
const fs = require("fs");
const zlib = require("zlib");
const path = require("path");

const SIZE = 1024;
const RADIUS = 180;
const CIRCLE_R = 300;

const lerp = (a, b, t) => a + (b - a) * t;
const hex = (h) => [
  parseInt(h.slice(1, 3), 16),
  parseInt(h.slice(3, 5), 16),
  parseInt(h.slice(5, 7), 16),
];

const c1 = hex("#8B6FBF");
const c2 = hex("#4A90D9");
const dot = hex("#F2F2F7");
const cx = SIZE / 2;
const cy = SIZE / 2;

function inRoundedRect(x, y) {
  const rx = Math.min(x, SIZE - 1 - x);
  const ry = Math.min(y, SIZE - 1 - y);
  if (rx < RADIUS && ry < RADIUS) {
    const dx = RADIUS - rx;
    const dy = RADIUS - ry;
    return Math.sqrt(dx * dx + dy * dy) <= RADIUS;
  }
  return true;
}

const raw = Buffer.alloc(SIZE * (1 + SIZE * 4));
let p = 0;
for (let y = 0; y < SIZE; y++) {
  raw[p++] = 0; // filter byte
  for (let x = 0; x < SIZE; x++) {
    let r = 0;
    let g = 0;
    let b = 0;
    let a = 0;
    if (inRoundedRect(x, y)) {
      const t = (x + y) / (2 * SIZE);
      r = Math.round(lerp(c1[0], c2[0], t));
      g = Math.round(lerp(c1[1], c2[1], t));
      b = Math.round(lerp(c1[2], c2[2], t));
      a = 255;
      const dist = Math.sqrt((x - cx) ** 2 + (y - cy) ** 2);
      if (dist <= CIRCLE_R) {
        const edge = 6;
        if (dist <= CIRCLE_R - edge) {
          r = dot[0];
          g = dot[1];
          b = dot[2];
        } else {
          const k = (CIRCLE_R - dist) / edge;
          r = Math.round(lerp(r, dot[0], k));
          g = Math.round(lerp(g, dot[1], k));
          b = Math.round(lerp(b, dot[2], k));
        }
      }
    }
    raw[p++] = r;
    raw[p++] = g;
    raw[p++] = b;
    raw[p++] = a;
  }
}

const crcTable = (() => {
  const t = [];
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++)
    c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const t = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])), 0);
  return Buffer.concat([len, t, data, crc]);
}

const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const idat = zlib.deflateSync(raw, { level: 9 });
const png = Buffer.concat([
  sig,
  chunk("IHDR", ihdr),
  chunk("IDAT", idat),
  chunk("IEND", Buffer.alloc(0)),
]);

const out = path.join(__dirname, "..", "icon-source.png");
fs.writeFileSync(out, png);
console.log("wrote", out, png.length, "bytes");
