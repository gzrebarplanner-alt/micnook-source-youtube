# YouTube 匯入來源（MicNook 擴充）

讓 MicNook 可以搜尋 YouTube，並下載影片的音訊與畫面來匯入歌曲。
這是**另外散布、使用者自己決定安裝**的擴充，不是 MicNook 本身的一部分；使用前請先看 [TERMS.md](TERMS.md)。

- 裝好後就在背景從 GitHub 的 [yt-dlp/yt-dlp](https://github.com/yt-dlp/yt-dlp) 下載 yt-dlp（Windows 是 `yt-dlp.exe`，Mac 是 `yt-dlp_macos`），放在 MicNook 給這個擴充的資料夾；之後每 3 天讓它自己更新一次
- 介面規格：MicNook 的 `docs/sources.md`（protocol 1）
- 這個擴充沒有 MicNook 的官方簽章，安裝時 App 會標示「未簽章」

## 打包

```bash
node pack.mjs
```

產生 `dist/micnook-source-youtube-<版本>.zip`，在 MicNook 的「設定 → 匯入來源 → 安裝來源…」選這個檔。
本機打包要先準備 Mac 版執行檔（見 pack.mjs 檔頭）。

## 發版

改好 `Cargo.toml`、`source.json` 的版本，寫 `release-notes/<版本>.md`，推 `v<版本>` 標籤：
GitHub Actions（`.github/workflows/release.yml`）會在 Mac 與 Windows 上各編一份、打包、發 Release，
再照執行結果 Summary 的 SHA-256 與大小更新 `catalog.json`。

原始碼與下載：<https://github.com/gzrebarplanner-alt/micnook-source-youtube>（Releases 放打包好的 zip）。
