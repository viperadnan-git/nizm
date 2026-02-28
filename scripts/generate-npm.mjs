#!/usr/bin/env node

// Generic npm package generator for shipping native binaries via npm.
// Produces a root package with a JS bin wrapper and per-platform packages.
//
// Usage:
//   node scripts/generate-npm.mjs [--version <ver>]
//
// Without --version, generates scaffolding with 0.0.0 placeholders.
// With --version, sets all package versions and chmod's any binaries found.
// ROOT is the directory from where the script is called (process.cwd()).

import { mkdirSync, writeFileSync, readFileSync, copyFileSync, chmodSync, existsSync, readdirSync } from "fs";
import { join } from "path";
import { parseArgs } from "util";

const ROOT = process.cwd();

const CONFIG = {
    name: "nizm-cli",
    bin: "nizm",
    scope: "@nizm-cli",
    description: "Lightweight, zero-config pre-commit hooks",
    license: "MIT",
    repository: "https://github.com/viperadnan-git/nizm",
    keywords: ["git", "hooks", "pre-commit", "linter", "formatter"],
    installHint: "npm install -g nizm-cli",
    readme: "README.md",
    targets: [
        { os: "linux",  cpu: "x64" },
        { os: "linux",  cpu: "arm64" },
        { os: "darwin", cpu: "x64" },
        { os: "darwin", cpu: "arm64" },
        { os: "win32",  cpu: "x64" },
    ],
};

const OS_DISPLAY = { linux: "linux", darwin: "macOS", win32: "Windows" };

const { values } = parseArgs({
    options: { version: { type: "string" } },
    strict: false,
});

const version = values.version || "0.0.0";

// --- Generators ---

function platformDirName(bin, target) {
    return `${bin}-${target.os === "darwin" ? "darwin" : target.os}-${target.cpu}`;
}

function generatePlatformPackage(outDir, config, target) {
    const { scope, bin } = config;
    const pkgName = `${scope}/${target.os === "darwin" ? "darwin" : target.os}-${target.cpu}`;
    const dirName = platformDirName(bin, target);
    const dir = join(outDir, dirName);

    mkdirSync(dir, { recursive: true });

    const pkg = {
        name: pkgName,
        version,
        description: `${bin} binary for ${OS_DISPLAY[target.os] || target.os} ${target.cpu}`,
        license: config.license,
        repository: { type: "git", url: config.repository },
        os: [target.os],
        cpu: [target.cpu],
    };

    writeFileSync(join(dir, "package.json"), JSON.stringify(pkg, null, 4) + "\n");

    // chmod any binaries already present (placed by CI before this script runs)
    for (const f of readdirSync(dir)) {
        if (f === "package.json") continue;
        try { chmodSync(join(dir, f), 0o755); } catch {}
    }

    return { pkgName, dirName };
}

function generateBinWrapper(config, platforms) {
    const entries = platforms
        .map((p) => `    "${p.target.os}-${p.target.cpu}": "${p.pkgName}"`)
        .join(",\n");

    const { bin, name } = config;
    const installHint = config.installHint || `npm install -g ${name}`;

    return `#!/usr/bin/env node

const PLATFORMS = {
${entries},
};

const key = \`\${process.platform}-\${process.arch}\`;
const pkg = PLATFORMS[key];

if (!pkg) {
    console.error(\`${bin}: unsupported platform \${key}\`);
    process.exit(1);
}

const ext = process.platform === "win32" ? ".exe" : "";
let bin;
try {
    bin = require.resolve(\`\${pkg}/${bin}\${ext}\`, { paths: [__dirname] });
} catch {
    console.error(
        \`${bin}: package \${pkg} not installed — reinstall with ${installHint}\`,
    );
    process.exit(1);
}

const result = require("child_process").spawnSync(bin, process.argv.slice(2), {
    shell: false,
    stdio: "inherit",
});

if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
`;
}

function generateRootPackage(outDir, config, platforms) {
    const { name, bin } = config;
    const dir = join(outDir, name);

    mkdirSync(join(dir, "bin"), { recursive: true });

    const optionalDependencies = {};
    for (const p of platforms) {
        optionalDependencies[p.pkgName] = version;
    }

    const files = [`bin/${bin}`];

    if (config.readme) {
        const readme = join(ROOT, config.readme);
        if (existsSync(readme)) {
            copyFileSync(readme, join(dir, "README.md"));
            files.push("README.md");
        }
    }

    const pkg = {
        name,
        version,
        description: config.description,
        license: config.license,
        repository: { type: "git", url: config.repository },
        keywords: config.keywords || [],
        bin: { [bin]: `bin/${bin}` },
        optionalDependencies,
        files,
    };

    writeFileSync(join(dir, "package.json"), JSON.stringify(pkg, null, 4) + "\n");
    writeFileSync(join(dir, "bin", bin), generateBinWrapper(config, platforms), { mode: 0o755 });
}

// --- Main ---

const outDir = join(ROOT, "npm");

const platforms = CONFIG.targets.map((target) => {
    const { pkgName, dirName } = generatePlatformPackage(outDir, CONFIG, target);
    return { target, pkgName, dirName };
});

generateRootPackage(outDir, CONFIG, platforms);

console.log(`Generated ${platforms.length + 1} npm packages in npm/ (v${version})`);
for (const p of platforms) {
    console.log(`  ${p.pkgName} -> npm/${p.dirName}/`);
}
console.log(`  ${CONFIG.name} -> npm/${CONFIG.name}/`);
