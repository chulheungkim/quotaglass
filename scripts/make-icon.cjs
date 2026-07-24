// Prepares the approved Meter Pane brand artwork for `tauri icon`.
// macOS optical sizing is intentionally preserved: the artwork occupies an
// 850x850 box centered on a transparent 1024x1024 canvas.
const { execFileSync } = require("child_process");
const { copyFileSync, existsSync, mkdtempSync, rmSync } = require("fs");
const os = require("os");
const path = require("path");

const ROOT = path.join(__dirname, "..");
const MASTER = path.join(
  ROOT,
  "assets",
  "brand",
  "quotaglass-meter-pane-master.png",
);
const OUTPUT = path.join(ROOT, "icon-source.png");

const CANVAS_SIZE = 1024;
const ARTWORK_SIZE = 850;
const SOURCE_CROP_SIZE = 950;
const SOURCE_CROP_OFFSET_Y = 149;
const SOURCE_CROP_OFFSET_X = 152;

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

  copyFileSync(padded, OUTPUT);
  console.log(
    `Wrote ${OUTPUT} (${ARTWORK_SIZE}px artwork on ${CANVAS_SIZE}px canvas)`,
  );
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true });
}
