const fs = require("fs");
const path = require("path");

const PLATFORMS = {
    "linux-x64": "@nizm/linux-x64",
    "linux-arm64": "@nizm/linux-arm64",
    "darwin-x64": "@nizm/darwin-x64",
    "darwin-arm64": "@nizm/darwin-arm64",
    "win32-x64": "@nizm/win32-x64",
};

const key = `${process.platform}-${process.arch}`;
const pkg = PLATFORMS[key];
if (!pkg) process.exit(0);

const ext = process.platform === "win32" ? ".exe" : "";
let src;
try {
    src = require.resolve(`${pkg}/nizm${ext}`);
} catch {
    // Platform package not installed — JS fallback wrapper remains
    process.exit(0);
}

const dest = path.join(__dirname, `nizm${ext}`);

try {
    // Remove the JS wrapper, replace with native binary
    fs.copyFileSync(src, dest);
    fs.chmodSync(dest, 0o755);
} catch {
    // Copy failed — JS fallback wrapper remains
}
