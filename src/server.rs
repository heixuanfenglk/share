use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Method, Request, StatusCode, Uri};
use axum::response::Response;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

const URL_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

#[derive(Clone)]
pub struct Share {
    pub id: String,
    pub root: PathBuf,
    pub label_path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone)]
struct AppState {
    shares: Arc<Vec<Share>>,
}

pub fn prepare_shares(paths: &[PathBuf]) -> Result<Vec<Share>, String> {
    if paths.is_empty() {
        return Err("请至少添加一个路径。".into());
    }
    let mut used = std::collections::HashSet::new();
    let mut shares = Vec::new();
    for path in paths {
        if !path.exists() {
            return Err(format!("路径不存在：{}", path.display()));
        }
        let root = path
            .canonicalize()
            .map_err(|err| format!("无法读取 {}：{err}", path.display()))?;
        if shares
            .iter()
            .any(|share: &Share| paths_equal(&share.root, &root))
        {
            return Err(format!("路径重复：{}", path.display()));
        }
        let is_dir = std::fs::metadata(path)
            .map(|meta| meta.is_dir())
            .unwrap_or_else(|_| root.is_dir());
        let id = unique_id(&label_for(path), &mut used);
        shares.push(Share {
            id,
            root,
            label_path: path.clone(),
            is_dir,
        });
    }
    Ok(shares)
}

pub fn router(shares: Vec<Share>) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(AppState {
        shares: Arc::new(shares),
    })
}

pub fn lan_urls(port: u16) -> Vec<String> {
    let mut ips = lan_ipv4();
    ips.sort_by(|left, right| rank(left).cmp(&rank(right)).then(left.cmp(right)));
    ips.dedup();
    ips.into_iter()
        .map(|ip| format!("http://{ip}:{port}/"))
        .collect()
}

fn lan_ipv4() -> Vec<String> {
    let Ok(list) = local_ip_address::list_afinet_netifas() else {
        return Vec::new();
    };
    list.into_iter()
        .filter_map(|(_name, ip)| match ip {
            std::net::IpAddr::V4(v4) if usable_v4(v4) => Some(v4.to_string()),
            _ => None,
        })
        .collect()
}

fn usable_v4(ip: Ipv4Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_link_local()
        && !ip.is_multicast()
        && !ip.is_broadcast()
}

fn rank(ip: &str) -> u8 {
    if ip.starts_with("192.168.") || ip.starts_with("10.") || is_private_172(ip) {
        0
    } else {
        1
    }
}

fn is_private_172(ip: &str) -> bool {
    let mut parts = ip.split('.');
    let Some(first) = parts.next().and_then(|part| part.parse::<u8>().ok()) else {
        return false;
    };
    let Some(second) = parts.next().and_then(|part| part.parse::<u8>().ok()) else {
        return false;
    };
    first == 172 && (16..=31).contains(&second)
}

async fn handle(State(state): State<AppState>, req: Request<Body>) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "只支持 GET。");
    }
    let decoded = decode_path(req.uri().path());
    let trimmed = decoded.trim_start_matches('/');
    if trimmed.is_empty() {
        return index_page(&state, &req);
    }
    let (id, rel) = match trimmed.split_once('/') {
        Some((id, rel)) => (id, rel.trim_matches('/')),
        None => (trimmed, ""),
    };
    if id.is_empty() || id == "." || id == ".." {
        return not_found("这个分享不存在，或链接已失效。");
    }
    let Some(share) = state.shares.iter().find(|share| share.id == id) else {
        return not_found("这个分享不存在，或链接已失效。");
    };

    if !share.is_dir {
        if !rel.is_empty() {
            return not_found("文件不存在。");
        }
        let query = file_query(&req);
        return serve_file(&share.root, query).await;
    }

    let Some(target) = safe_join(&share.root, rel) else {
        return not_found("路径不存在，或超出了分享目录。");
    };
    if target.is_dir() {
        if !req.uri().path().ends_with('/') {
            return redirect_slash(req.uri());
        }
        let host = host_of(&req);
        return list_dir(share, rel, &target, host).await;
    }
    let query = file_query(&req);
    serve_file(&target, query).await
}

struct FileQuery {
    head: bool,
    download: bool,
    range: Option<String>,
}

fn file_query(req: &Request<Body>) -> FileQuery {
    FileQuery {
        head: req.method() == Method::HEAD,
        download: wants_download(req.uri()),
        range: req
            .headers()
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    }
}

async fn list_dir(share: &Share, rel: &str, dir: &Path, host: String) -> Response {
    let root = share.root.clone();
    let rel_owned = rel.to_string();
    let dir_owned = dir.to_path_buf();
    let listed =
        tokio::task::spawn_blocking(move || collect_entries(&root, &rel_owned, &dir_owned)).await;
    let entries = match listed {
        Ok(Ok(entries)) => entries,
        Ok(Err(err)) => {
            return html_error(
                StatusCode::FORBIDDEN,
                "无法打开文件夹",
                &format!("没有权限读取这个目录。{err}"),
                &host,
            );
        }
        Err(_) => {
            return html_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "读取失败",
                "列出目录时出错。",
                &host,
            );
        }
    };
    let body = render_listing(share, rel, &entries);
    html(StatusCode::OK, &share.id, &body, &host)
}

fn collect_entries(root: &Path, rel: &str, dir: &Path) -> Result<Vec<Entry>, std::io::Error> {
    let mut items = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':', '\0']) {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if safe_join(root, &child_rel).is_none() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        items.push(Entry {
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: meta.modified().ok(),
        });
    }
    items.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(items)
}

struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

async fn serve_file(path: &Path, query: FileQuery) -> Response {
    let meta = match tokio::fs::metadata(path).await {
        Ok(meta) if meta.is_file() => meta,
        _ => return not_found("文件不存在或已被移走。"),
    };
    let total = meta.len();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());
    let download = query.download;
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type(path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_DISPOSITION,
            content_disposition(&name, download),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff");

    if total == 0 {
        builder = builder
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, 0);
        return builder
            .body(Body::empty())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "响应失败。"));
    }

    let (start, end, partial) = if let Some(spec) = query.range.as_deref() {
        match parse_range(spec, total) {
            Some((start, end)) => (start, end, true),
            None => return range_not_satisfiable(total),
        }
    } else {
        (0, total - 1, false)
    };
    let len = end - start + 1;
    builder = builder
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_LENGTH, len);
    if partial {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    if query.head {
        return builder
            .body(Body::empty())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "响应失败。"));
    }

    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => return not_found("文件不存在或已被移走。"),
    };
    if start > 0 && file.seek(SeekFrom::Start(start)).await.is_err() {
        return html_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "读取失败",
            "无法读取这个文件。",
            "",
        );
    }
    let limited = file.take(len);
    let stream = ReaderStream::with_capacity(limited, 64 * 1024);
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "响应失败。"))
}

fn index_page(state: &AppState, req: &Request<Body>) -> Response {
    let mut cards = String::new();
    if state.shares.is_empty() {
        cards.push_str(
            r#"<div class="empty-pick"><p class="empty-title">还没有配置分享路径</p><p>请在分享程序里选择文件夹。未选择前，这里不会列出当前目录。</p></div>"#,
        );
    }
    for share in state.shares.iter() {
        let href = if share.is_dir {
            format!("/{}/", encode_component(&share.id))
        } else {
            format!("/{}", encode_component(&share.id))
        };
        let kind = if share.is_dir { "文件夹" } else { "文件" };
        let action = if share.is_dir { "打开" } else { "下载" };
        cards.push_str(&format!(
            r#"<a class="share" href="{href}"><span class="kind">{kind}</span><strong>{}</strong><span class="go">{action}</span></a>"#,
            escape(&share.id)
        ));
    }
    let count = state.shares.len();
    let sub = if count == 0 {
        "请先在本机选择要分享的文件夹。".to_string()
    } else {
        format!("{count} 个位置可以浏览和下载。")
    };
    let body = format!(
        r#"<header class="mast"><p class="eyebrow">Local network</p><h1>共享目录</h1><p class="sub">{sub}</p><div class="mark" aria-hidden="true">内网<span>共享</span></div></header><section class="stack">{cards}</section>"#
    );
    let host = host_of(req);
    html(StatusCode::OK, "共享目录", &body, &host)
}

fn render_listing(share: &Share, rel: &str, entries: &[Entry]) -> String {
    let parts: Vec<&str> = rel.split('/').filter(|part| !part.is_empty()).collect();
    let here = if parts.is_empty() {
        share.id.clone()
    } else {
        format!("{} / {}", share.id, parts.join(" / "))
    };
    let up_href = if parts.is_empty() {
        "/".to_string()
    } else if parts.len() == 1 {
        format!("/{}/", encode_component(&share.id))
    } else {
        format!(
            "/{}/{}/",
            encode_component(&share.id),
            encoded_rel(&parts[..parts.len() - 1])
        )
    };
    let up_label = if parts.is_empty() {
        "全部分享"
    } else {
        "上一级"
    };
    let mut rows = format!(
        r#"<a class="row up" href="{up_href}"><span class="name">{up_label}</span><span class="meta">返回</span><span class="go">打开</span></a>"#
    );
    if entries.is_empty() {
        rows.push_str(r#"<p class="empty">这个文件夹是空的。</p>"#);
    }
    for entry in entries {
        let encoded = encode_component(&entry.name);
        let name = escape(&entry.name);
        if entry.is_dir {
            rows.push_str(&format!(
                r#"<div class="row"><a class="name" href="{encoded}/">{name}</a><span class="meta">文件夹 · {}</span><a class="open" href="{encoded}/">打开</a></div>"#,
                format_mtime(entry.modified)
            ));
        } else {
            rows.push_str(&format!(
                r#"<div class="row"><a class="name" href="{encoded}">{name}</a><span class="meta">{} · {}</span><a class="dl" href="{encoded}?download=1" download aria-label="下载 {name}">下载</a></div>"#,
                format_size(entry.size),
                format_mtime(entry.modified)
            ));
        }
    }
    format!(
        r#"<header class="mast"><p class="eyebrow">Local network</p><h1>{}</h1><p class="sub">{} 项</p><div class="mark" aria-hidden="true">内网<span>共享</span></div></header>{}<section class="stack">{rows}</section>"#,
        escape(&here),
        entries.len(),
        breadcrumbs(share, &parts)
    )
}

fn breadcrumbs(share: &Share, parts: &[&str]) -> String {
    let mut html = format!(
        r#"<nav class="crumb"><a href="/">全部分享</a><span class="sep">/</span><a href="/{}/">{}</a>"#,
        encode_component(&share.id),
        escape(&share.id)
    );
    for (index, part) in parts.iter().enumerate() {
        html.push_str(r#"<span class="sep">/</span>"#);
        if index + 1 == parts.len() {
            html.push_str(&format!("<span>{}</span>", escape(part)));
        } else {
            html.push_str(&format!(
                r#"<a href="/{}/{}">{}</a>"#,
                encode_component(&share.id),
                encoded_rel(&parts[..=index]),
                escape(part)
            ));
        }
    }
    html.push_str("</nav>");
    html
}

fn html(status: StatusCode, title: &str, body: &str, host: &str) -> Response {
    let page = render_page(title, body, host);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CONTENT_LENGTH, page.len())
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header(
            "Content-Security-Policy",
            "default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:; base-uri 'none'; form-action 'none'",
        )
        .body(Body::from(page))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "响应失败。"))
}

fn html_error(status: StatusCode, title: &str, message: &str, host: &str) -> Response {
    let body = format!(
        r#"<header class="mast"><p class="eyebrow">{code}</p><h1>{title}</h1><p class="sub">{message}</p></header><p><a class="open" href="/">回到共享列表</a></p>"#,
        code = status.as_u16(),
        title = escape(title),
        message = escape(message)
    );
    html(status, title, &body, host)
}

fn not_found(message: &str) -> Response {
    html_error(StatusCode::NOT_FOUND, "找不到", message, "")
}

fn plain(status: StatusCode, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(message.to_string()))
        .unwrap_or_else(|_| Response::new(Body::from(message.to_string())))
}

fn redirect_slash(uri: &Uri) -> Response {
    let location = match uri.query() {
        Some(query) => format!("{}/?{query}", uri.path()),
        None => format!("{}/", uri.path()),
    };
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, location)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::empty())
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "响应失败。"))
}

fn range_not_satisfiable(size: u64) -> Response {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::CONTENT_RANGE, format!("bytes */{size}"))
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::empty())
        .unwrap_or_else(|_| plain(StatusCode::RANGE_NOT_SATISFIABLE, "范围无效。"))
}

fn wants_download(uri: &Uri) -> bool {
    uri.query()
        .map(|query| {
            query
                .split('&')
                .any(|pair| pair.split('=').next().unwrap_or("").eq("download"))
        })
        .unwrap_or(false)
}

fn host_of(req: &Request<Body>) -> String {
    req.headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn content_type(path: &Path) -> String {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let subtype = mime.subtype().as_str();
    if mime.type_().as_str() == "text" || matches!(subtype, "json" | "javascript" | "xml") {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    }
}

fn content_disposition(name: &str, attachment: bool) -> String {
    let kind = if attachment { "attachment" } else { "inline" };
    let ascii: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii() && !matches!(ch, '"' | '\\' | '\r' | '\n') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let encoded = utf8_percent_encode(name, URL_COMPONENT);
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

fn parse_range(header: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let spec = header.trim().strip_prefix("bytes=")?.trim();
    if spec.is_empty() || spec.contains(',') {
        return None;
    }
    let (start_text, end_text) = spec.split_once('-')?;
    if start_text.is_empty() {
        let suffix: u64 = end_text.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let suffix = suffix.min(size);
        Some((size - suffix, size - 1))
    } else {
        let start: u64 = start_text.parse().ok()?;
        if start >= size {
            return None;
        }
        let end = if end_text.is_empty() {
            size - 1
        } else {
            let end: u64 = end_text.parse().ok()?;
            if end < start {
                return None;
            }
            end.min(size - 1)
        };
        Some((start, end))
    }
}

fn decode_path(path: &str) -> String {
    percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .into_owned()
}

fn encode_component(text: &str) -> String {
    utf8_percent_encode(text, URL_COMPONENT).to_string()
}

fn encoded_rel(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| encode_component(part))
        .collect::<Vec<_>>()
        .join("/")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn format_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_mtime(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return "—".into();
    };
    let local = chrono::DateTime::<chrono::Local>::from(time);
    local.format("%Y-%m-%d %H:%M").to_string()
}

fn label_for(path: &Path) -> String {
    if let Some(name) = path.file_name() {
        let text = name.to_string_lossy();
        if !text.is_empty() {
            return text.into_owned();
        }
    }
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        "share".into()
    } else {
        trimmed.to_string()
    }
}

fn unique_id(name: &str, used: &mut std::collections::HashSet<String>) -> String {
    let mut base = name.trim().replace(['/', '\\'], "-");
    if base.is_empty() || base == "." || base == ".." {
        base = "share".into();
    }
    let mut id = base.clone();
    let mut suffix = 2u32;
    while used.contains(&id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    used.insert(id.clone());
    id
}

pub fn paths_equal(left: &Path, right: &Path) -> bool {
    norm_key(left) == norm_key(right)
}

fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for segment in rel.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains(':') || segment.contains('\0') {
            return None;
        }
        out.push(segment);
    }
    let canon = out.canonicalize().ok()?;
    let root_canon = root.canonicalize().ok()?;
    if !is_within(&root_canon, &canon) {
        return None;
    }
    Some(canon)
}

fn is_within(root: &Path, target: &Path) -> bool {
    let root = norm_key(root);
    let target = norm_key(target);
    if target == root {
        return true;
    }
    let prefix = if root.ends_with('\\') {
        root
    } else {
        format!("{root}\\")
    };
    target.starts_with(&prefix)
}

fn norm_key(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('/', "\\");
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        text = format!(r"\\{rest}");
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        text = rest.to_string();
    }
    let is_drive_root =
        text.len() == 3 && text.as_bytes().get(1) == Some(&b':') && text.ends_with('\\');
    if !is_drive_root {
        while text.ends_with('\\') {
            text.pop();
        }
    }
    text.to_lowercase()
}

fn render_page(title: &str, body: &str, host: &str) -> String {
    let host_line = if host.is_empty() {
        "仅供局域网内访问".to_string()
    } else {
        format!("来自 {} · 仅供局域网内访问", escape(host))
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
:root {{
  color-scheme: light;
  --bg: #e5d8c4;
  --paper: rgba(247, 241, 230, 0.92);
  --ink: #1c1712;
  --muted: #6d6256;
  --line: rgba(74, 52, 32, 0.16);
  --stamp: #9d3b2b;
  --accent: #0f5e55;
  --accent-ink: #f3fffc;
}}
* {{ box-sizing: border-box; }}
body {{
  margin: 0;
  min-height: 100vh;
  color: var(--ink);
  font-family: "Avenir Next", "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif;
  background:
    radial-gradient(900px 420px at -10% -20%, rgba(255, 248, 236, 0.95), transparent 60%),
    radial-gradient(680px 360px at 110% -10%, rgba(157, 59, 43, 0.16), transparent 55%),
    linear-gradient(180deg, #f3eadc 0%, var(--bg) 42%, #d9c7ae 100%);
}}
main {{
  width: min(920px, calc(100% - 28px));
  margin: 0 auto;
  padding: 36px 0 72px;
}}
.mast {{ position: relative; padding: 8px 120px 8px 0; margin-bottom: 18px; }}
.eyebrow {{
  margin: 0 0 8px;
  color: var(--stamp);
  letter-spacing: 0.18em;
  text-transform: uppercase;
  font-size: 12px;
}}
h1 {{
  margin: 0;
  font-family: "Iowan Old Style", "Palatino Linotype", Palatino, "Songti SC", "Noto Serif SC", serif;
  font-size: clamp(34px, 6vw, 56px);
  font-weight: 600;
  letter-spacing: -0.035em;
  line-height: 0.95;
}}
.sub {{ margin: 10px 0 0; color: var(--muted); }}
.mark {{
  position: absolute;
  right: 0;
  top: 0;
  width: 96px;
  height: 96px;
  border: 2px solid var(--stamp);
  border-radius: 50%;
  color: var(--stamp);
  display: grid;
  place-items: center;
  text-align: center;
  transform: rotate(12deg);
  letter-spacing: 0.08em;
  font-family: "Palatino Linotype", Palatino, "Songti SC", serif;
  font-size: 14px;
  line-height: 1.35;
}}
.mark span {{ display: block; }}
.crumb {{
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
  margin: 0 0 14px;
  color: var(--muted);
}}
.crumb a {{ color: var(--accent); text-decoration: none; }}
.sep {{ opacity: 0.55; }}
.stack {{ display: flex; flex-direction: column; gap: 8px; }}
.share, .row {{
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  gap: 12px 16px;
  align-items: center;
  padding: 14px 16px;
  text-decoration: none;
  color: inherit;
  background: var(--paper);
  border: 1px solid var(--line);
  border-radius: 16px;
  box-shadow: 0 10px 30px rgba(58, 36, 14, 0.04);
  animation: rise 0.28s ease;
}}
.share {{ grid-template-columns: auto minmax(0, 1fr) auto; }}
.share strong, .name {{
  font-size: 17px;
  font-weight: 650;
  overflow-wrap: anywhere;
}}
.name {{ color: inherit; text-decoration: none; }}
.name:hover, .share:hover strong {{ color: var(--accent); }}
.kind, .meta {{ color: var(--muted); font-size: 13px; font-variant-numeric: tabular-nums; }}
.kind {{ letter-spacing: 0.08em; }}
.go, .open, .dl {{
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 64px;
  min-height: 36px;
  padding: 0 12px;
  border-radius: 999px;
  text-decoration: none;
  font-size: 13px;
}}
.go, .dl {{ background: var(--accent); color: var(--accent-ink); }}
.open {{ color: var(--accent); border: 1px solid rgba(15, 94, 85, 0.35); }}
.empty {{ color: var(--muted); padding: 12px 4px; }}
.empty-pick {{
  padding: 28px 22px;
  background: var(--paper);
  border: 1px dashed rgba(157, 59, 43, 0.55);
  border-radius: 16px;
}}
.empty-pick .empty-title {{
  margin: 0 0 8px;
  color: var(--ink);
  font-size: 22px;
  font-weight: 650;
}}
.empty-pick p {{ margin: 0; color: var(--muted); font-size: 16px; line-height: 1.5; }}
footer {{ margin-top: 28px; color: var(--muted); font-size: 13px; }}
a:focus-visible {{ outline: 2px solid var(--accent); outline-offset: 3px; }}
@keyframes rise {{
  from {{ opacity: 0; transform: translateY(8px); }}
  to {{ opacity: 1; transform: none; }}
}}
@media (max-width: 640px) {{
  .mast {{ padding-right: 0; }}
  .mark {{ display: none; }}
  .row {{ grid-template-columns: minmax(0, 1fr) auto; }}
  .meta {{ grid-column: 1 / -1; }}
}}
@media (prefers-reduced-motion: reduce) {{
  .share, .row {{ animation: none; }}
}}
</style>
</head>
<body>
<main>
{body}
<footer>{host_line}</footer>
</main>
</body>
</html>"#,
        title = escape(title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct TempTree {
        root: PathBuf,
        outside: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "lan-share-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("子目录")).unwrap();
            std::fs::write(root.join("hello.txt"), b"0123456789").unwrap();
            std::fs::write(root.join("子目录").join("你好.txt"), "内容").unwrap();
            let outside = std::env::temp_dir().join(format!(
                "lan-share-secret-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::write(&outside, b"secret").unwrap();
            Self { root, outside }
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_file(&self.outside);
        }
    }

    async fn body_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn get(app: &axum::Router, uri: &str) -> Response {
        app.clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[test]
    fn rejects_paths_outside_the_share() {
        let tree = TempTree::new();
        let root = tree.root.canonicalize().unwrap();
        assert!(safe_join(&root, "hello.txt").is_some());
        assert!(safe_join(&root, "../").is_none());
        assert!(safe_join(&root, r"..\..\windows").is_none());
        assert!(safe_join(&root, "hello.txt:stream").is_none());
        let sibling = tree.root.with_file_name(format!(
            "{}-extra",
            tree.root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(!is_within(&root, &sibling.canonicalize().unwrap()));
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn parses_byte_ranges() {
        assert_eq!(parse_range("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(parse_range("bytes=2-", 10), Some((2, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=10-20", 10), None);
        assert_eq!(parse_range("bytes=0-1,2-3", 10), None);
    }

    #[tokio::test]
    async fn serves_listing_and_file_without_escaping_the_root() {
        let tree = TempTree::new();
        let shares = prepare_shares(&[tree.root.clone()]).unwrap();
        let id = shares[0].id.clone();
        let app = router(shares);

        let index = get(&app, "/").await;
        assert_eq!(index.status(), StatusCode::OK);
        let index_body = body_string(index).await;
        assert!(index_body.contains(&id));
        assert!(!index_body.contains("secret"));

        let listing = get(&app, &format!("/{id}/")).await;
        assert_eq!(listing.status(), StatusCode::OK);
        let listing_body = body_string(listing).await;
        assert!(listing_body.contains("hello.txt"));
        assert!(listing_body.contains("子目录"));
        assert!(listing_body.contains("download=1"));

        let file = get(&app, &format!("/{id}/hello.txt")).await;
        assert_eq!(file.status(), StatusCode::OK);
        assert_eq!(
            file.headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(body_string(file).await, "0123456789");

        let named = get(
            &app,
            &format!("/{id}/%E5%AD%90%E7%9B%AE%E5%BD%95/%E4%BD%A0%E5%A5%BD.txt"),
        )
        .await;
        assert_eq!(named.status(), StatusCode::OK);
        assert_eq!(body_string(named).await, "内容");

        let download = get(&app, &format!("/{id}/hello.txt?download=1")).await;
        let disposition = download
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(disposition.starts_with("attachment;"));

        let partial = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/{id}/hello.txt"))
                    .header(header::RANGE, "bytes=0-3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(body_string(partial).await, "0123");

        let escaped = get(&app, &format!("/{id}/%2e%2e/%2e%2e/windows")).await;
        assert_eq!(escaped.status(), StatusCode::NOT_FOUND);
        let slash = get(&app, &format!("/{id}/%5c..%5c..%5cwindows")).await;
        assert_eq!(slash.status(), StatusCode::NOT_FOUND);

        let redirect = get(&app, &format!("/{id}/{}", encode_component("子目录"))).await;
        assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
        assert!(redirect
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with('/'));
    }
}
