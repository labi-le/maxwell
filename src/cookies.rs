use crate::prelude::*;

// Read the original bind-mounted jar, never the mutable yt-dlp request copy.
// Invalid or oversized jars fail closed without exposing their contents.
pub(crate) fn tiktok_page_cookies(
    source: Option<&Path>,
    page: &Url,
) -> Result<Option<String>, String> {
    let Some(source) = source else {
        return Ok(None);
    };
    let error = || "Не удалось подготовить cookies".to_string();
    const MAX_JAR_BYTES: u64 = 4 * 1024 * 1024;
    const MAX_HEADER_BYTES: usize = 8192;
    if fs::metadata(source).map_err(|_| error())?.len() > MAX_JAR_BYTES {
        return Err(error());
    }
    let file = fs::File::open(source).map_err(|_| error())?;
    let mut bytes = Vec::new();
    file.take(MAX_JAR_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| error())?;
    let jar = std::str::from_utf8(&bytes).map_err(|_| error())?;
    let mut lines = jar.lines();
    if lines.next().map(|line| line.trim_end_matches('\r')) != Some("# Netscape HTTP Cookie File") {
        return Err(error());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error())?
        .as_secs();
    let mut header = String::new();
    let mut found = false;
    let mut auth_found = false;
    for line in lines {
        let row = line.trim_end_matches('\r');
        if row.is_empty() || (row.starts_with('#') && !row.starts_with("#HttpOnly_")) {
            continue;
        }
        let mut fields = row.split('\t');
        let (
            Some(domain),
            Some(subdomains),
            Some(path),
            Some(secure),
            Some(expiry),
            Some(name),
            Some(value),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            return Err(error());
        };
        let domain = domain.strip_prefix("#HttpOnly_").unwrap_or(domain);
        let host_matches = if subdomains == "TRUE" {
            let domain = domain.strip_prefix('.').unwrap_or(domain);
            domain.eq_ignore_ascii_case("tiktok.com")
                || domain.eq_ignore_ascii_case("www.tiktok.com")
        } else {
            domain.eq_ignore_ascii_case("www.tiktok.com")
        };
        if !host_matches {
            continue;
        }
        let request_path = page.path();
        let path_matches = request_path.starts_with(path)
            && (path.ends_with('/')
                || request_path.len() == path.len()
                || request_path.as_bytes().get(path.len()) == Some(&b'/'));
        if !path_matches {
            continue;
        }
        let valid_field = |field: &str| {
            field
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b';' | b',' | b'\\' | b'"'))
        };
        let valid_name =
            |byte: u8| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
        if fields.next().is_some()
            || domain.is_empty()
            || !valid_field(domain)
            || !valid_field(path)
            || !path.starts_with('/')
            || !valid_field(expiry)
            || !name.bytes().all(valid_name)
            || name.is_empty()
            || !matches!(subdomains, "TRUE" | "FALSE")
            || !matches!(secure, "TRUE" | "FALSE")
        {
            return Err(error());
        }
        // Browsers store auxiliary cookies such as g_state as JSON. Their value
        // cannot be sent verbatim in a Cookie header, but it must not make the
        // whole jar unusable when the auth cookies are valid.
        if !value.bytes().all(|byte| {
            (0x21..=0x7e).contains(&byte) && !matches!(byte, b';' | b',' | b'\\' | b'"')
        }) {
            continue;
        }
        let expires = expiry.parse::<u64>().map_err(|_| error())?;
        found = true;
        if secure == "TRUE" && page.scheme() != "https" || (expires != 0 && expires <= now) {
            continue;
        }
        if matches!(name, "sessionid" | "sessionid_ss") {
            auth_found = true;
        }
        let extra = name.len() + value.len() + if header.is_empty() { 1 } else { 3 };
        if header.len() + extra > MAX_HEADER_BYTES {
            return Err(error());
        }
        if !header.is_empty() {
            header.push_str("; ");
        }
        header.push_str(name);
        header.push('=');
        header.push_str(value);
    }
    if !found || !auth_found {
        return Err(error());
    }
    Ok((!header.is_empty()).then_some(header))
}

// Validate the source without creating a temporary copy. The preflight uses this
// to distinguish an unusable jar from a storage failure in the work directory.
pub(crate) fn validate_cookie_jar(source: &Path) -> Result<(), String> {
    let error = || "Не удалось подготовить cookies".to_string();
    let file = fs::File::open(source).map_err(|_| error())?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|_| error())?;
    if line.trim_end_matches(['\r', '\n']) != "# Netscape HTTP Cookie File" {
        return Err(error());
    }
    let mut found = false;
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|_| error())? == 0 {
            break;
        }
        let row = line.trim_end_matches(['\r', '\n']);
        if row.is_empty() || (row.starts_with('#') && !row.starts_with("#HttpOnly_")) {
            continue;
        }
        let mut fields = row.split('\t');
        let (
            Some(host),
            Some(domain),
            Some(path),
            Some(secure),
            Some(expiry),
            Some(name),
            Some(_value),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            return Err(error());
        };
        if fields.next().is_some()
            || host.is_empty()
            || !path.starts_with('/')
            || name.is_empty()
            || !matches!(domain, "TRUE" | "FALSE")
            || !matches!(secure, "TRUE" | "FALSE")
            || expiry.parse::<u64>().is_err()
        {
            return Err(error());
        }
        found = true;
    }
    if !found {
        return Err(error());
    }
    Ok(())
}

// Jar parsing touches a bind mount synchronously, so callers inside async
// tasks hand it to a blocking worker instead of stalling the runtime.
async fn blocking_cookies<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(task)
        .await
        .unwrap_or_else(|_| Err("Не удалось подготовить cookies".to_string()))
}

pub(crate) async fn process_cookies_async(
    source: Option<&Path>,
    dir: &Path,
) -> Result<Option<(tempfile::TempDir, PathBuf)>, String> {
    let source = source.map(Path::to_path_buf);
    let dir = dir.to_path_buf();
    blocking_cookies(move || process_cookies(source.as_deref(), &dir)).await
}

pub(crate) async fn validate_cookie_jar_async(source: &Path) -> Result<(), String> {
    let source = source.to_path_buf();
    blocking_cookies(move || validate_cookie_jar(&source)).await
}

pub(crate) async fn tiktok_page_cookies_async(
    source: Option<&Path>,
    page: &Url,
) -> Result<Option<String>, String> {
    let source = source.map(Path::to_path_buf);
    let page = page.clone();
    blocking_cookies(move || tiktok_page_cookies(source.as_deref(), &page)).await
}

// Each subprocess owns a writable jar inside the request's private directory.
// The bind-mounted source is opened only for reading and is never passed to yt-dlp.
pub(crate) fn process_cookies(
    source: Option<&Path>,
    dir: &Path,
) -> Result<Option<(tempfile::TempDir, PathBuf)>, String> {
    let Some(source) = source else {
        return Ok(None);
    };
    validate_cookie_jar(source)?;
    let error = || "Не удалось подготовить cookies".to_string();
    let mut file = fs::File::open(source).map_err(|_| error())?;
    let temp = tempfile::Builder::new()
        .prefix("cookies-")
        .tempdir_in(dir)
        .map_err(|_| error())?;
    let path = temp.path().join("cookies.txt");
    let mut copy = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| error())?;
    std::io::copy(&mut file, &mut copy).map_err(|_| error())?;
    Ok(Some((temp, path)))
}
