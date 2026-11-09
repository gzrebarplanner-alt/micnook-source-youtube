// 發版的第二段：GitHub Actions 發的是草稿 Release（release.yml），在 Windows 上把 zip 裡的 source-youtube.exe
// 補上程式碼簽章、換回草稿、再發佈。最後印出 catalog.json 要填的 SHA-256 與大小（catalog.json 還是手動改）。
//
//   node sign-release.mjs <版本>     例如 node sign-release.mjs 1.0.8
//
// 起因：Windows 11 的「智慧型應用程式控制」擋掉沒有簽章的 source-youtube.exe（os error 4551，MicNook 回報 d497aa48）。
// 簽章借用 MicNook 的 app/scripts/sign.mjs（Certum 雲端憑證）：要先打開 SimplySign Desktop、用手機 App 的動態碼登入。
// GitHub 要用 gzrebarplanner-alt 的帳號（gh auth switch -u gzrebarplanner-alt），做完切回 Gargion。
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO = "gzrebarplanner-alt/micnook-source-youtube";
const APP_SIGN = path.join(DIR, "..", "Any KAREOKIE", "app", "scripts", "sign.mjs");
const { certAvailable, sign, verify } = await import(pathToFileURL(APP_SIGN).href);

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("用法：node sign-release.mjs <版本>");
  process.exit(1);
}
const manifest = JSON.parse(fs.readFileSync(path.join(DIR, "source.json"), "utf8"));
if (manifest.version !== version) {
  console.error(`source.json 的版本是 ${manifest.version}，不是 ${version}`);
  process.exit(1);
}
if (!certAvailable()) {
  console.error("找不到簽章憑證：先打開 SimplySign Desktop 登入（右下角圖示 → 連線，輸入手機 App 的動態碼）");
  process.exit(1);
}

const sh = (cmd, args) => {
  const r = spawnSync(cmd, args, { stdio: "inherit", shell: false });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(" ")} 失敗`);
};
const ps = (script) => sh("powershell", ["-NoProfile", "-Command", script]);

const tag = `v${version}`;
const name = `micnook-source-youtube-${version}.zip`;
const work = fs.mkdtempSync(path.join(os.tmpdir(), "sign-source-"));
const zip = path.join(work, name);
const stage = path.join(work, "stage");
try {
  sh("gh", ["release", "download", tag, "-R", REPO, "-p", name, "-D", work]);
  ps(`Expand-Archive -LiteralPath '${zip}' -DestinationPath '${stage}'`);
  const exe = path.join(stage, manifest.exec);
  sign(exe);
  verify(exe);
  fs.rmSync(zip);
  ps(`Compress-Archive -Path '${stage}\\*' -DestinationPath '${zip}'`);
  sh("gh", ["release", "upload", tag, zip, "-R", REPO, "--clobber"]);
  sh("gh", ["release", "edit", tag, "-R", REPO, "--draft=false"]);
  const buf = fs.readFileSync(zip);
  console.log(`\n已簽章並發佈 ${tag}。catalog.json：`);
  console.log(`  download: https://github.com/${REPO}/releases/download/${tag}/${name}`);
  console.log(`  sha256:   ${crypto.createHash("sha256").update(buf).digest("hex")}`);
  console.log(`  size:     ${buf.length}`);
} finally {
  fs.rmSync(work, { recursive: true, force: true });
}
