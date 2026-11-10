//! MicNook 的 YouTube 匯入來源（protocol 1，規格見 MicNook 的 docs/sources.md）。
//! 從標準輸入讀一行請求，把它翻成 yt-dlp 指令；進度與結果一行一個 JSON 寫到標準輸出。
//! yt-dlp 第一次用時從 GitHub 下載到 App 給的 dataDir，之後每 3 天讓它自己更新一次。

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime};

/// yt-dlp：Mac 是官方的單一執行檔 yt-dlp_macos（Universal，Apple 晶片也能跑）。
/// Windows 與 Android 用 Python 原始碼的 zipapp（yt-dlp），交給 Python 執行（command）：
/// Windows 以前用 yt-dlp.exe，它沒有程式碼簽章，Windows 11 的「智慧型應用程式控制」會擋（os error 4551，MicNook 回報 d497aa48），
/// 改用 python.org 的 embeddable Python（Python Software Foundation 簽章）來跑；zipapp 不是執行檔，不受它管
#[cfg(any(windows, target_os = "android"))]
const YTDLP: &str = "yt-dlp";
/// Windows 的 Python：固定版本，下載後核對 SHA-256（2026-11-09 從 python.org 下載、檔案都有 PSF 的有效簽章）
#[cfg(windows)]
const PYTHON_ZIP: &str = "https://www.python.org/ftp/python/3.14.10/python-3.14.10-embed-amd64.zip";
#[cfg(windows)]
const PYTHON_SHA256: &str = "90905e64a840557d8c1bc4f5b74db3d7ee85e3ffb217611cac1c7813acea4159";
#[cfg(not(any(windows, target_os = "android")))]
const YTDLP: &str = "yt-dlp_macos";
const UPDATE_EVERY: Duration = Duration::from_secs(3 * 24 * 3600);

struct Ctx {
    data: PathBuf,
    ffmpeg: Option<String>,
    zh: bool,
}

/// 錯誤：代碼＋給使用者看的訊息
struct Fail(&'static str, String);
type R<T> = Result<T, Fail>;

fn out(v: Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn main() {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let req: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    let locale = req["locale"].as_str().unwrap_or("zh-TW").to_string();
    let ctx = Ctx {
        // 沒給資料夾（直接手動執行時）就放暫存資料夾，不要放在目前的資料夾
        data: req["dataDir"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("any-kareokie-source-youtube")),
        ffmpeg: req["ffmpeg"].as_str().map(str::to_string),
        zh: locale.starts_with("zh"),
    };
    let args = &req["args"];
    let r = match req["op"].as_str().unwrap_or("") {
        "setup" => setup(&ctx).map(|_| json!({})),
        "search" => search(&ctx, args),
        "meta" => meta(&ctx, args),
        "playlist" => playlist(&ctx, args),
        "audio" => audio(&ctx, args),
        "video" => video(&ctx, args),
        op => Err(Fail("unsupported", format!("unsupported op: {op}"))),
    };
    match r {
        Ok(v) => out(json!({ "result": v })),
        Err(Fail(code, message)) => out(json!({ "error": { "code": code, "message": message } })),
    }
}

impl Ctx {
    fn t(&self, zh: &str, en: &str) -> String {
        if self.zh { zh.into() } else { en.into() }
    }
    fn ytdlp(&self) -> PathBuf {
        self.data.join(YTDLP)
    }
}

// ---------------------------------------------------------------- yt-dlp 本身

/// 沒有 yt-dlp 就下載；有的話超過 3 天沒更新就讓它自己更新（失敗不影響這次使用）
fn setup(ctx: &Ctx) -> R<()> {
    let exe = ctx.ytdlp();
    let stamp = ctx.data.join("updated-at");
    #[cfg(windows)]
    if !python_of(&exe).is_file() {
        std::fs::create_dir_all(&ctx.data).map_err(|e| Fail("setup_failed", e.to_string()))?;
        download_python(ctx, &exe.with_file_name("python"))?;
        // 1.0.7 以前的 yt-dlp.exe 用不到了
        let _ = std::fs::remove_file(ctx.data.join("yt-dlp.exe"));
    }
    if !exe.is_file() {
        std::fs::create_dir_all(&ctx.data).map_err(|e| Fail("setup_failed", e.to_string()))?;
        download_ytdlp(ctx, &exe)?;
        touch(&stamp);
        return Ok(());
    }
    let old = std::fs::metadata(&stamp).and_then(|m| m.modified()).map(|t| SystemTime::now().duration_since(t).unwrap_or_default() > UPDATE_EVERY).unwrap_or(true);
    if old {
        out(json!({ "progress": 0.0, "message": ctx.t("更新 yt-dlp…", "Updating yt-dlp…") }));
        let _ = run(&exe, &["-U"]);
        touch(&stamp);
    }
    Ok(())
}

/// 記下「剛更新過」：寫進現在的時間。以前寫空字串，檔案本來就是空的時 Windows 不改修改時間，
/// 超過 3 天之後每次開 App 第一次用都要等一次更新
fn touch(stamp: &Path) {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = std::fs::write(stamp, now.to_string());
    if let Ok(f) = std::fs::File::options().write(true).open(stamp) {
        let _ = f.set_modified(SystemTime::now());
    }
}

fn download_ytdlp(ctx: &Ctx, exe: &Path) -> R<()> {
    let fail = |e: String| Fail("setup_failed", format!("{}{e}", ctx.t("下載 yt-dlp 失敗：", "Couldn't download yt-dlp: ")));
    let tmp = exe.with_extension("download");
    download(ctx, &format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{YTDLP}"), &tmp, "yt-dlp").map_err(fail)?;
    // Mac：下載下來沒有執行權限
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| fail(e.to_string()))?;
    }
    std::fs::rename(&tmp, exe).map_err(|e| fail(e.to_string()))
}

/// Windows：yt-dlp 旁邊的 python 資料夾裡的 python.exe
#[cfg(windows)]
fn python_of(ytdlp: &Path) -> PathBuf {
    ytdlp.with_file_name("python").join("python.exe")
}

/// Windows：下載 embeddable Python、核對 SHA-256、解開到 dir（先解到暫存資料夾，好了才改名，中斷不會留下半套）
#[cfg(windows)]
fn download_python(ctx: &Ctx, dir: &Path) -> R<()> {
    use sha2::{Digest, Sha256};
    let fail = |e: String| Fail("setup_failed", format!("{}{e}", ctx.t("下載 Python 失敗：", "Couldn't download Python: ")));
    let zip_path = dir.with_extension("zip.download");
    download(ctx, PYTHON_ZIP, &zip_path, "Python").map_err(fail)?;
    let bytes = std::fs::read(&zip_path).map_err(|e| fail(e.to_string()))?;
    let _ = std::fs::remove_file(&zip_path);
    let hash: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if hash != PYTHON_SHA256 {
        return Err(fail(ctx.t("檔案檢查碼不符", "checksum mismatch")));
    }
    let tmp = dir.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    zip::ZipArchive::new(std::io::Cursor::new(bytes)).and_then(|mut z| z.extract(&tmp)).map_err(|e| fail(e.to_string()))?;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir).map_err(|e| fail(e.to_string()))
}

/// 下載到 dest，一邊印進度（「下載 <label> N%」）
fn download(ctx: &Ctx, url: &str, dest: &Path, label: &str) -> Result<(), String> {
    let resp = ureq::get(url).call().map_err(|e| e.to_string())?;
    let total: f64 = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok()).unwrap_or(18e6);
    let mut body = resp.into_body();
    let mut reader = body.as_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let (mut got, mut last) = (0usize, 0.0f64);
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n;
        let p = (got as f64 / total).min(1.0);
        if p - last >= 0.05 {
            last = p;
            out(json!({ "progress": p, "message": format!("{} {label} {:.0}%", ctx.t("下載", "Downloading"), p * 100.0) }));
        }
    }
    Ok(())
}

/// Windows：Python 跑 yt-dlp 的 zipapp（見 YTDLP）
#[cfg(windows)]
fn command(exe: &Path, args: &[&str]) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(python_of(exe));
    cmd.arg(exe);
    // yt-dlp 是 Python 程式：強制 UTF-8，否則中日文標題在 Windows 上會變亂碼
    cmd.args(args).env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    cmd.creation_flags(0x0800_0000);
    cmd
}

#[cfg(not(any(windows, target_os = "android")))]
fn command(exe: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args).env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    cmd
}

/// Android：App（MicNook 的 pyrt.rs）用環境變數給 Python 的位置（MICNOOK_PYTHON，其他 Python 要的環境變數也設好了）
/// 與 yt-dlp 解 YouTube JS 驗證用的執行環境（MICNOOK_JS_RUNTIME，例如 quickjs:/路徑）
#[cfg(target_os = "android")]
fn command(exe: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(std::env::var_os("MICNOOK_PYTHON").unwrap_or_else(|| "python3".into()));
    cmd.arg(exe);
    if let Ok(js) = std::env::var("MICNOOK_JS_RUNTIME") {
        cmd.args(["--no-js-runtimes", "--js-runtimes", &js]);
    }
    cmd.args(args).env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    cmd
}

fn run(exe: &Path, args: &[&str]) -> std::io::Result<Output> {
    command(exe, args).output()
}

/// 開不了 yt-dlp（或 Python）：被 Windows 的應用程式控制擋下（os error 4551）時說清楚，其他照原樣
fn spawn_fail(ctx: &Ctx, e: std::io::Error) -> Fail {
    if e.raw_os_error() == Some(4551) {
        return Fail("blocked", ctx.t("Windows 的安全設定（智慧型應用程式控制）擋下了下載程式", "Windows security settings (Smart App Control) blocked the downloader"));
    }
    Fail("setup_failed", e.to_string())
}

/// 跑 yt-dlp；失敗時把 stderr 翻成錯誤代碼
fn ytdlp(ctx: &Ctx, args: &[&str]) -> R<Output> {
    let o = run(&ctx.ytdlp(), args).map_err(|e| spawn_fail(ctx, e))?;
    if o.status.success() {
        return Ok(o);
    }
    Err(classify(ctx, &String::from_utf8_lossy(&o.stderr)))
}

fn classify(ctx: &Ctx, err: &str) -> Fail {
    let e = err.to_lowercase();
    let tail = err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or(err.lines().last().unwrap_or("")).trim().to_string();
    eprintln!("{err}");
    // HTTP 錯誤碼先看：「unable to download … HTTP Error 416」不是網路斷了
    let http = e.split("http error ").nth(1).and_then(|s| s.get(..3)).and_then(|s| s.parse::<u16>().ok());
    if let Some(status) = http {
        return match status {
            403 | 429 => Fail("network", ctx.t("YouTube 暫時不讓下載，等幾分鐘再試一次", "YouTube is temporarily refusing downloads. Try again in a few minutes.")),
            416 => Fail("internal", ctx.t("下載的暫存檔對不上，請再匯入一次", "A leftover partial download got in the way. Please import again.")),
            500..=599 => Fail("network", ctx.t("YouTube 暫時有問題，等一下再試", "YouTube is having trouble right now. Try again shortly.")),
            404 | 410 => Fail("not_found", ctx.t("找不到這支影片（可能已刪除或設為私人）", "This video isn't available (it may have been removed or made private)")),
            _ => Fail("internal", tail),
        };
    }
    // 「Sign in to confirm you're not a bot」：YouTube 把這個網路當成機器人，跟影片無關（以前被說成影片有觀看限制，MicNook 0.1.37 的回報）。
    // 撇號可能是彎的、或被解碼成亂碼，只比 not a bot
    if e.contains("not a bot") {
        return Fail("network", ctx.t("YouTube 暫時把你的網路當成機器人，不讓下載。請過一陣子再試，或換個網路（例如關掉 VPN、改用手機熱點）", "YouTube is treating your network as a bot right now. Try again later, or switch networks (for example, turn off your VPN)."));
    }
    if e.contains("is live") || e.contains("live event") {
        Fail("live", ctx.t("直播無法匯入", "Live streams can't be imported"))
    } else if e.contains("video unavailable") || e.contains("is unavailable") || e.contains("not available") || e.contains("removed") || e.contains("does not exist") {
        Fail("not_found", ctx.t("找不到這支影片（可能已刪除或設為私人）", "This video isn't available (it may have been removed or made private)"))
    } else if e.contains("sign in") || e.contains("your age") || e.contains("age-restricted") || e.contains("age restricted") || e.contains("private") || e.contains("country") || e.contains("region") {
        Fail("unavailable", ctx.t("這支影片有觀看限制（登入、年齡或地區）", "This video is restricted (sign-in, age or region)"))
    } else if e.contains("unable to download") || e.contains("timed out") || e.contains("connection") || e.contains("getaddrinfo") {
        Fail("network", ctx.t("連不上 YouTube，請確認網路", "Couldn't reach YouTube. Check your connection."))
    } else {
        Fail("internal", tail)
    }
}

fn parse(ctx: &Ctx, o: &Output) -> R<Value> {
    serde_json::from_slice(&o.stdout).map_err(|_| Fail("internal", ctx.t("yt-dlp 的回應不是 JSON", "yt-dlp returned something unexpected")))
}

// ---------------------------------------------------------------- 動作

/// "yt:XXXX" → 網址；網址照原樣
fn url_of(input: &str) -> String {
    match input.strip_prefix("yt:") {
        Some(id) => format!("https://www.youtube.com/watch?v={id}"),
        None => input.to_string(),
    }
}

fn search(ctx: &Ctx, a: &Value) -> R<Value> {
    let query = a["query"].as_str().unwrap_or("").trim();
    let offset = a["offset"].as_u64().unwrap_or(0) as usize;
    let limit = a["limit"].as_u64().unwrap_or(12).clamp(1, 50) as usize;
    if query.is_empty() {
        return Ok(json!({ "hits": [], "more": false }));
    }
    let items = format!("{}:{}", offset + 1, offset + limit);
    let o = ytdlp(ctx, &["-J", "--flat-playlist", "--no-warnings", "--playlist-items", &items, &format!("ytsearch{}:{query}", offset + limit)])?;
    let j = parse(ctx, &o)?;
    let entries = j["entries"].as_array().cloned().unwrap_or_default();
    let more = entries.len() >= limit;
    // 直播、預告不列
    let hits: Vec<Value> = entries
        .iter()
        .filter(|e| e["live_status"].as_str().map(|s| s == "not_live" || s == "was_live").unwrap_or(true))
        .filter_map(|e| {
            let id = e["id"].as_str()?;
            Some(json!({
                "ref": format!("yt:{id}"),
                "title": e["title"].as_str().unwrap_or_default(),
                "channel": e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or_default(),
                "duration": e["duration"].as_f64().unwrap_or(0.0),
                "url": format!("https://www.youtube.com/watch?v={id}"),
                "thumbnail": format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"),
            }))
        })
        .collect();
    Ok(json!({ "hits": hits, "more": more }))
}

/// 播放清單網址裡的歌（最多 PLAYLIST_MAX 首）；私人、已刪除的影片與直播不列
const PLAYLIST_MAX: usize = 200;
fn playlist(ctx: &Ctx, a: &Value) -> R<Value> {
    let url = a["input"].as_str().unwrap_or("").trim().to_string();
    let items = format!("1:{PLAYLIST_MAX}");
    let o = ytdlp(ctx, &["-J", "--flat-playlist", "--yes-playlist", "--no-warnings", "--playlist-items", &items, &url])?;
    let j = parse(ctx, &o)?;
    if j["_type"].as_str() != Some("playlist") {
        return Err(Fail("not_found", ctx.t("這不是播放清單", "This isn't a playlist")));
    }
    let entries = j["entries"].as_array().cloned().unwrap_or_default();
    let hits: Vec<Value> = entries
        .iter()
        .filter(|e| e["live_status"].as_str().map(|s| s == "not_live" || s == "was_live").unwrap_or(true))
        .filter(|e| e["availability"].as_str().map(|s| s != "private" && s != "needs_auth").unwrap_or(true))
        .filter_map(|e| {
            let id = e["id"].as_str()?;
            let title = e["title"].as_str().unwrap_or_default();
            // 已刪除、私人影片：yt-dlp 只給「[Deleted video]」「[Private video]」這種標題、沒有長度
            if title.starts_with('[') && title.ends_with(']') && e["duration"].is_null() {
                return None;
            }
            Some(json!({
                "ref": format!("yt:{id}"),
                "title": title,
                "channel": e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or_default(),
                "duration": e["duration"].as_f64().unwrap_or(0.0),
                "url": format!("https://www.youtube.com/watch?v={id}"),
                "thumbnail": format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"),
            }))
        })
        .collect();
    let total = j["playlist_count"].as_u64().unwrap_or(entries.len() as u64);
    Ok(json!({ "title": j["title"].as_str().unwrap_or_default(), "hits": hits, "total": total }))
}

fn meta(ctx: &Ctx, a: &Value) -> R<Value> {
    let url = url_of(a["input"].as_str().unwrap_or(""));
    let o = ytdlp(ctx, &["-J", "--no-playlist", "--no-warnings", &url])?;
    let j = parse(ctx, &o)?;
    if j["is_live"].as_bool().unwrap_or(false) {
        return Err(Fail("live", ctx.t("直播無法匯入", "Live streams can't be imported")));
    }
    let s = |k: &str| j[k].as_str().map(str::to_string);
    let id = s("id").ok_or_else(|| Fail("not_found", ctx.t("沒有影片 id", "No video id")))?;
    Ok(json!({
        "ref": format!("yt:{id}"),
        "title": s("title").unwrap_or_default(),
        "uploader": s("channel").or_else(|| s("uploader")).unwrap_or_default(),
        "track": s("track"),
        "artist": s("artist").or_else(|| s("creator")),
        "duration": j["duration"].as_f64().unwrap_or(0.0),
        "thumbnail": s("thumbnail"),
        "url": s("webpage_url").unwrap_or(url),
    }))
}

/// 下載到 dir，一邊把 yt-dlp 的進度轉成 progress 行；回傳下載好的檔案（檔名開頭 stem）
fn fetch(ctx: &Ctx, dir: &Path, stem: &str, format: &[&str], url: &str) -> R<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| Fail("internal", e.to_string()))?;
    let template = dir.join(format!("{stem}.%(ext)s"));
    let template = template.to_string_lossy();
    let mut args: Vec<&str> = format.to_vec();
    // --no-continue：資料夾裡有上次留下的同名檔（--no-part 直接寫進正式檔名）也從頭下載，不續傳（續傳會遇到 416）
    args.extend(["--no-playlist", "--no-warnings", "--no-part", "--no-continue", "--newline", "--progress-template", "download:%(progress._percent_str)s", "-o", &template]);
    if let Some(f) = &ctx.ffmpeg {
        args.extend(["--ffmpeg-location", f]);
    }
    args.push(url);
    let mut child = command(&ctx.ytdlp(), &args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| spawn_fail(ctx, e))?;
    let mut stderr = child.stderr.take().unwrap();
    let errs = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    });
    progress_lines(child.stdout.take().unwrap(), |p| out(json!({ "progress": p })));
    let status = child.wait().map_err(|e| Fail("internal", e.to_string()))?;
    let err = errs.join().unwrap_or_default();
    if !status.success() {
        return Err(classify(ctx, &err));
    }
    std::fs::read_dir(dir)
        .map_err(|e| Fail("internal", e.to_string()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_file() && p.file_stem().map(|s| s == stem).unwrap_or(false))
        .ok_or_else(|| Fail("internal", ctx.t("yt-dlp 沒有產生檔案", "yt-dlp didn't produce a file")))
}

/// 讀 yt-dlp 的標準輸出直到結束，進度行（--progress-template 的「N.N%」）換成 0～1 交給 f。
/// 一定要讀到底、不能因為某一行不是 UTF-8 就停：yt-dlp.exe 不理 PYTHONUTF8，「[download] Destination: 路徑」
/// 用系統碼頁印（中文 Windows 是 cp950）。以前用 lines() 遇到就停止讀取、管線關掉，yt-dlp 下一次印進度就
/// 「Unable to download …: [Errno 22] Invalid argument」，資料位置含中文的人匯入一律失敗、還被說成連不上 YouTube（0.1.36 的回報）
fn progress_lines(r: impl Read, mut f: impl FnMut(f64)) {
    let mut r = BufReader::new(r);
    let mut buf = Vec::new();
    while r.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
        if let Ok(p) = String::from_utf8_lossy(&buf).trim().trim_end_matches('%').trim().parse::<f64>() {
            f((p / 100.0).clamp(0.0, 1.0));
        }
        buf.clear();
    }
}

/// 最高位元率的音訊（不重新編碼）
fn audio(ctx: &Ctx, a: &Value) -> R<Value> {
    let dir = PathBuf::from(a["dir"].as_str().unwrap_or("."));
    let p = fetch(ctx, &dir, "source", &["-f", "bestaudio/best"], &url_of(a["ref"].as_str().unwrap_or("")))?;
    Ok(json!({ "path": p }))
}

/// 不含聲音的影片：maxHeight 以下最高的解析度，同解析度挑檔案最小的（通常是 AV1，其次 VP9、H.264）
fn video(ctx: &Ctx, a: &Value) -> R<Value> {
    let dir = PathBuf::from(a["dir"].as_str().unwrap_or("."));
    let h = a["maxHeight"].as_u64().unwrap_or(720);
    let (f, s) = (format!("bv*[height<={h}]/bv*"), format!("res:{h},+size,+br"));
    let p = fetch(ctx, &dir, "video", &["-f", &f, "-S", &s], &url_of(a["ref"].as_str().unwrap_or("")))?;
    Ok(json!({ "path": p }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(err: &str) -> &'static str {
        classify(&Ctx { data: PathBuf::new(), ffmpeg: None, zh: true }, err).0
    }

    #[test]
    fn 進度_遇到不是_utf8_的行也讀到底() {
        // 「資料」的 cp950：b8 ea ae c6（yt-dlp.exe 在中文 Windows 印 Destination 的實際內容）
        // 進度行是 --progress-template 印的「  N.N%」；最後一行可能沒有換行
        let s = b"  0.0%\n[download] Destination: C:\\\xb8\xea\xae\xc6\\source.webm\n 55.5%\n100.0%";
        let mut got = vec![];
        progress_lines(&s[..], |p| got.push(p));
        assert_eq!(got, vec![0.0, 0.555, 1.0]);
    }

    #[test]
    fn 錯誤分類() {
        // 續傳殘檔：不是網路問題（2026-10-29 被說成「連不上 YouTube」）
        assert_eq!(code("ERROR: unable to download video data: HTTP Error 416: Requested range not satisfiable"), "internal");
        // YouTube 暫時擋下載：等一下再試會好，算 network
        assert_eq!(code("ERROR: unable to download video data: HTTP Error 403: Forbidden"), "network");
        assert_eq!(code("ERROR: Unable to download webpage: HTTP Error 429: Too Many Requests"), "network");
        // 「webpage」裡有 age，不能當成年齡限制
        assert_eq!(code("ERROR: Unable to download webpage: <urlopen error [Errno 11001] getaddrinfo failed>"), "network");
        assert_eq!(code("ERROR: [youtube] abc: Read timed out."), "network");
        assert_eq!(code("ERROR: [youtube] abc: Sign in to confirm your age. This video may be inappropriate for some users."), "unavailable");
        // YouTube 把網路當成機器人：不是影片的觀看限制
        assert_eq!(code("ERROR: [youtube] abc: Sign in to confirm you\u{2019}re not a bot. Use --cookies-from-browser or --cookies for the authentication."), "network");
        assert_eq!(code("ERROR: [youtube] abc: Sign in to confirm you\u{fffd}\u{fffd}re not a bot."), "network");
        assert_eq!(code("ERROR: [youtube] abc: Video unavailable"), "not_found");
        assert_eq!(code("ERROR: [youtube] abc: This live event will begin in 3 hours."), "live");
    }
}

#[cfg(test)]
mod stamp_tests {
    use super::*;

    #[test]
    fn 記下更新時間會改到修改時間() {
        let p = std::env::temp_dir().join(format!("ak-stamp-{}", std::process::id()));
        std::fs::write(&p, "").unwrap();
        let old = SystemTime::now() - Duration::from_secs(5 * 24 * 3600);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
        touch(&p);
        let m = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert!(SystemTime::now().duration_since(m).unwrap() < Duration::from_secs(60));
        let _ = std::fs::remove_file(&p);
    }
}
