// Prepares the approved Meter Pane brand artwork for `tauri icon`.
// macOS optical sizing is intentionally preserved: the artwork occupies an
// 850x850 box centered on a transparent 1024x1024 canvas.
const { execFileSync } = require("child_process");
const {
  copyFileSync,
  existsSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} = require("fs");
const os = require("os");
const path = require("path");
const zlib = require("zlib");

const ROOT = path.join(__dirname, "..");
const MASTER = path.join(
  ROOT,
  "assets",
  "brand",
  "quotaglass-meter-pane-master.png",
);
const APP_ICON_OUTPUT = path.join(ROOT, "icon-source.png");
const TRAY_SVG_OUTPUT = path.join(
  ROOT,
  "assets",
  "brand",
  "quotaglass-meter-stack.svg",
);
const TRAY_PNG_OUTPUT = path.join(ROOT, "src-tauri", "icons", "tray-icon.png");
const TRAY_RUST_OUTPUT = path.join(ROOT, "src-tauri", "src", "tray_icon.rs");

const CANVAS_SIZE = 1024;
const ARTWORK_SIZE = 850;
const SOURCE_CROP_SIZE = 950;
const SOURCE_CROP_OFFSET_Y = 149;
const SOURCE_CROP_OFFSET_X = 152;
const TRAY_ICON_SIZE = 32;
const TRAY_SUPERSAMPLE = 8;
const METER_BARS = [
  { x: 4, y: 6, width: 24, height: 5, radius: 2.5 },
  { x: 4, y: 13.5, width: 19, height: 5, radius: 2.5 },
  { x: 4, y: 21, width: 14, height: 5, radius: 2.5 },
];

if (!existsSync(MASTER)) {
  throw new Error(`Missing brand master: ${MASTER}`);
}

const temporaryDirectory = mkdtempSync(
  path.join(os.tmpdir(), "quotaglass-icon-"),
);
const cropped = path.join(temporaryDirectory, "cropped.png");
const resized = path.join(temporaryDirectory, "resized.png");
const padded = path.join(temporaryDirectory, "padded.png");

function runSips(arguments_) {
  execFileSync("sips", arguments_, { stdio: "inherit" });
}

function isInsideRoundedRectangle(x, y, rectangle) {
  const nearestX = Math.max(
    rectangle.x + rectangle.radius,
    Math.min(x, rectangle.x + rectangle.width - rectangle.radius),
  );
  const nearestY = Math.max(
    rectangle.y + rectangle.radius,
    Math.min(y, rectangle.y + rectangle.height - rectangle.radius),
  );
  const deltaX = x - nearestX;
  const deltaY = y - nearestY;

  return deltaX * deltaX + deltaY * deltaY <= rectangle.radius ** 2;
}

function createTrayRgba() {
  const rgba = Buffer.alloc(TRAY_ICON_SIZE * TRAY_ICON_SIZE * 4);
  const samplesPerPixel = TRAY_SUPERSAMPLE ** 2;

  for (let y = 0; y < TRAY_ICON_SIZE; y += 1) {
    for (let x = 0; x < TRAY_ICON_SIZE; x += 1) {
      let coveredSamples = 0;

      for (let sampleY = 0; sampleY < TRAY_SUPERSAMPLE; sampleY += 1) {
        for (let sampleX = 0; sampleX < TRAY_SUPERSAMPLE; sampleX += 1) {
          const pointX = x + (sampleX + 0.5) / TRAY_SUPERSAMPLE;
          const pointY = y + (sampleY + 0.5) / TRAY_SUPERSAMPLE;
          if (
            METER_BARS.some((rectangle) =>
              isInsideRoundedRectangle(pointX, pointY, rectangle),
            )
          ) {
            coveredSamples += 1;
          }
        }
      }

      const offset = (y * TRAY_ICON_SIZE + x) * 4;
      rgba[offset] = 0;
      rgba[offset + 1] = 0;
      rgba[offset + 2] = 0;
      rgba[offset + 3] = Math.round((coveredSamples / samplesPerPixel) * 255);
    }
  }

  return rgba;
}

const crcTable = (() => {
  const table = [];
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    table[index] = value >>> 0;
  }
  return table;
})();

function crc32(buffer) {
  let value = 0xffffffff;
  for (const byte of buffer) {
    value = crcTable[(value ^ byte) & 0xff] ^ (value >>> 8);
  }
  return (value ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const typeBuffer = Buffer.from(type, "ascii");
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(Buffer.concat([typeBuffer, data])));
  return Buffer.concat([length, typeBuffer, data, checksum]);
}

function encodePng(rgba, size) {
  const raw = Buffer.alloc(size * (1 + size * 4));
  let rawOffset = 0;
  let rgbaOffset = 0;

  for (let y = 0; y < size; y += 1) {
    raw[rawOffset] = 0;
    rawOffset += 1;
    rgba.copy(raw, rawOffset, rgbaOffset, rgbaOffset + size * 4);
    rawOffset += size * 4;
    rgbaOffset += size * 4;
  }

  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8;
  header[9] = 6;

  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function createTraySvg() {
  const rectangles = METER_BARS.map(
    ({ x, y, width, height, radius }) =>
      `    <rect x="${x}" y="${y}" width="${width}" height="${height}" rx="${radius}"/>`,
  ).join("\n");

  return [
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">',
    '  <g fill="#000">',
    rectangles,
    "  </g>",
    "</svg>",
    "",
  ].join("\n");
}

function createTrayRust(rgba) {
  const rows = [];
  for (let offset = 0; offset < rgba.length; offset += 32) {
    rows.push(
      `    ${Array.from(rgba.subarray(offset, offset + 32)).join(", ")},`,
    );
  }

  return [
    "// Auto-generated by scripts/make-icon.cjs — do not edit manually.",
    "// Meter Stack (32×32 RGBA, black on transparent) for macOS template mode.",
    `pub const TRAY_ICON_SIZE: u32 = ${TRAY_ICON_SIZE};`,
    "#[rustfmt::skip]",
    "pub const TRAY_ICON_RGBA: &[u8] = &[",
    ...rows,
    "];",
    "",
  ].join("\n");
}

function writeTrayAssets() {
  const rgba = createTrayRgba();
  writeFileSync(TRAY_SVG_OUTPUT, createTraySvg());
  writeFileSync(TRAY_PNG_OUTPUT, encodePng(rgba, TRAY_ICON_SIZE));
  writeFileSync(TRAY_RUST_OUTPUT, createTrayRust(rgba));
  console.log(`Wrote ${TRAY_SVG_OUTPUT}`);
  console.log(`Wrote ${TRAY_PNG_OUTPUT}`);
  console.log(`Wrote ${TRAY_RUST_OUTPUT}`);
}

try {
  runSips([
    "--cropToHeightWidth",
    String(SOURCE_CROP_SIZE),
    String(SOURCE_CROP_SIZE),
    "--cropOffset",
    String(SOURCE_CROP_OFFSET_Y),
    String(SOURCE_CROP_OFFSET_X),
    MASTER,
    "--out",
    cropped,
  ]);

  runSips([
    "--resampleHeightWidth",
    String(ARTWORK_SIZE),
    String(ARTWORK_SIZE),
    cropped,
    "--out",
    resized,
  ]);

  runSips([
    "--padToHeightWidth",
    String(CANVAS_SIZE),
    String(CANVAS_SIZE),
    resized,
    "--out",
    padded,
  ]);

  copyFileSync(padded, APP_ICON_OUTPUT);
  console.log(
    `Wrote ${APP_ICON_OUTPUT} (${ARTWORK_SIZE}px artwork on ${CANVAS_SIZE}px canvas)`,
  );
  writeTrayAssets();
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true });
}
