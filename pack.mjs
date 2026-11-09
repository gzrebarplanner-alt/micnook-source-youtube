// 打包：cargo build --release → dist/micnook-source-youtube-<版本>.zip（source.json、Windows 與 Mac 的執行檔、README、TERMS、LICENSE）。
// 這裡不簽章：GitHub Actions 發草稿後，在 Windows 上用 sign-release.mjs 簽 source-youtube.exe 再發佈。zip 用 PowerShell 的 Compress-Archive，不另外裝套件。
// Mac（Apple 晶片）的執行檔要在 Mac 上編：cargo build --release --target aarch64-apple-darwin，
// 把 target/aarch64-apple-darwin/release/source-youtube 放到 dist/mac/source-youtube-macos（或用環境變數 MAC_BIN 指定）
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const DIR = path.dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(fs.readFileSync(path.join(DIR, "source.json"), "utf8"));
const target = process.env.CARGO_TARGET_DIR ?? path.join(DIR, "target");
const cargo = process.platform === "win32" ? path.join(os.homedir(), ".cargo", "bin", "cargo.exe") : "cargo";

const b = spawnSync(cargo, ["build", "--release"], { cwd: DIR, stdio: "inherit", env: { ...process.env, CARGO_TARGET_DIR: target } });
if (b.status !== 0) process.exit(1);

const stage = path.join(DIR, "dist", "stage");
fs.rmSync(stage, { recursive: true, force: true });
fs.mkdirSync(stage, { recursive: true });
fs.copyFileSync(path.join(target, "release", "source-youtube.exe"), path.join(stage, manifest.exec));
const macBin = process.env.MAC_BIN ?? path.join(DIR, "dist", "mac", "source-youtube-macos");
if (!fs.existsSync(macBin)) {
  console.error(`找不到 Mac 版執行檔 ${macBin}（見檔頭）`);
  process.exit(1);
}
fs.copyFileSync(macBin, path.join(stage, manifest.execMacos));
for (const f of ["source.json", "README.md", "TERMS.md", "LICENSE"]) fs.copyFileSync(path.join(DIR, f), path.join(stage, f));

const zip = path.join(DIR, "dist", `micnook-source-youtube-${manifest.version}.zip`);
fs.rmSync(zip, { force: true });
const z = spawnSync("powershell", ["-NoProfile", "-Command", `Compress-Archive -Path '${stage}\\*' -DestinationPath '${zip}'`], { stdio: "inherit" });
if (z.status !== 0) process.exit(1);
fs.rmSync(stage, { recursive: true, force: true });
console.log(`已打包：${zip}`);
