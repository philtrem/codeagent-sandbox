#!/usr/bin/env node
//
// Build the sandbox binary and guest VM images, then copy them to
// the Tauri bundle directories (sidecar + resources).
//
// Usage:
//   node desktop/scripts/copy-sidecar.js           # release build
//   node desktop/scripts/copy-sidecar.js --debug    # debug build

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");

const REPO_ROOT = path.resolve(__dirname, "../..");
const BINARIES_DIR = path.join(REPO_ROOT, "desktop/src-tauri/binaries");
const RESOURCES_DIR = path.join(REPO_ROOT, "desktop/src-tauri/resources/guest");

const debug = process.argv.includes("--debug");
const profile = debug ? "debug" : "release";

function run(cmd) {
  console.log(`> ${cmd}`);
  execSync(cmd, { stdio: "inherit", cwd: REPO_ROOT });
}

function tryRun(cmd) {
  try {
    execSync(cmd, { stdio: "inherit", cwd: REPO_ROOT });
    return true;
  } catch {
    return false;
  }
}

function commandExists(cmd) {
  try {
    execSync(os.platform() === "win32" ? `where ${cmd}` : `which ${cmd}`, {
      stdio: "ignore",
    });
    return true;
  } catch {
    return false;
  }
}

// Detect target triple and guest architecture
const platform = os.platform();
const arch = os.arch();
let target, guestArch;

if (platform === "linux" && arch === "x64") {
  target = "x86_64-unknown-linux-gnu";
  guestArch = "x86_64";
} else if (platform === "linux" && arch === "arm64") {
  target = "aarch64-unknown-linux-gnu";
  guestArch = "aarch64";
} else if (platform === "darwin" && arch === "x64") {
  target = "x86_64-apple-darwin";
  guestArch = "x86_64";
} else if (platform === "darwin" && arch === "arm64") {
  target = "aarch64-apple-darwin";
  guestArch = "aarch64";
} else if (platform === "win32" && arch === "x64") {
  target = "x86_64-pc-windows-msvc";
  guestArch = "x86_64";
} else {
  console.error(`Unsupported platform: ${platform}-${arch}`);
  process.exit(1);
}

// --- Sandbox binary ---

console.log(`Building sandbox binary (${profile}, ${target})...`);
if (profile === "release") {
  run("cargo build --release -p codeagent-sandbox");
} else {
  run("cargo build -p codeagent-sandbox");
}

const sourceDir = path.join(REPO_ROOT, "target", profile);
fs.mkdirSync(BINARIES_DIR, { recursive: true });

const ext = platform === "win32" ? ".exe" : "";
const srcName = `sandbox${ext}`;
const dstName = `sandbox-${target}${ext}`;

fs.copyFileSync(path.join(sourceDir, srcName), path.join(BINARIES_DIR, dstName));
console.log(`Copied sidecar: ${path.join(BINARIES_DIR, dstName)}`);

// --- Guest VM images ---

const guestDir = path.join(REPO_ROOT, "target/guest", guestArch);
const vmlinuzPath = path.join(guestDir, "vmlinuz");
const initrdPath = path.join(guestDir, "initrd.img");

if (!fs.existsSync(vmlinuzPath) || !fs.existsSync(initrdPath)) {
  if (commandExists("docker")) {
    console.log(`\nBuilding guest VM image (${guestArch})...`);
    const xtaskManifest = path.join(REPO_ROOT, "xtask/Cargo.toml");
    if (
      !tryRun(`cargo xtask build-guest --arch ${guestArch}`) &&
      !tryRun(
        `cargo run --manifest-path ${xtaskManifest} -- build-guest --arch ${guestArch}`
      )
    ) {
      console.warn(
        "warning: guest image build failed (Docker may not be running)"
      );
    }
  } else {
    console.warn("\nwarning: Docker not found — skipping guest image build.");
    console.warn(
      "  Run 'cargo xtask build-guest' manually, then re-run this script."
    );
  }
}

fs.mkdirSync(RESOURCES_DIR, { recursive: true });

if (fs.existsSync(vmlinuzPath) && fs.existsSync(initrdPath)) {
  fs.copyFileSync(vmlinuzPath, path.join(RESOURCES_DIR, "vmlinuz"));
  fs.copyFileSync(initrdPath, path.join(RESOURCES_DIR, "initrd.img"));
  console.log(`Copied guest images: ${RESOURCES_DIR}/{vmlinuz,initrd.img}`);
} else {
  console.warn(`\nwarning: Guest images not found at ${guestDir}`);
  console.warn(
    "  The desktop app will work but cannot start the VM without kernel/initrd."
  );
  console.warn(`  Build them with: cargo xtask build-guest --arch ${guestArch}`);
}
