use crate::prelude::*;
use crate::*;

pub(crate) struct CachedPinnedClient {
    pub(crate) host: String,
    pub(crate) addresses: Vec<SocketAddr>,
    pub(crate) client: reqwest::Client,
}

pub(crate) static PINNED_CLIENTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<IpAddr, CachedPinnedClient>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

// Pin resolved addresses and disable redirects for both photos and short links.
pub(crate) async fn pinned_client(url: &Url) -> Result<reqwest::Client, String> {
    let host = url.host_str().ok_or("Отсутствует имя HTTPS-хоста")?;
    let addresses: Vec<SocketAddr> = lookup_host((host, 443))
        .await
        .map_err(|_| "Не удалось разрешить имя HTTPS-хоста")?
        .filter(|address| permitted_ipv4(address.ip()))
        .collect();
    if addresses.is_empty() {
        return Err("HTTPS-хост не имеет пригодного IPv4-адреса".into());
    }
    let ip = addresses[0].ip();
    let mut cache = PINNED_CLIENTS.lock().ok();
    if let Some(cached) = cache.as_ref().and_then(|cache| cache.get(&ip)) {
        // A host-specific DNS override cannot safely be reused for another host
        // or after DNS removes one of its previously pinned addresses.
        if cached.host == host && cached.addresses == addresses {
            return Ok(cached.client.clone());
        }
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .https_only(true)
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| "Не удалось настроить безопасное HTTPS-подключение".to_string())?;
    if let Some(cache) = cache.as_mut() {
        cache.insert(
            ip,
            CachedPinnedClient {
                host: host.to_owned(),
                addresses,
                client: client.clone(),
            },
        );
    }
    Ok(client)
}

pub(crate) async fn save_image_response(
    mut response: reqwest::Response,
    dir: &Path,
    name: &str,
    max_bytes: u64,
    total_remaining: u64,
    expected_ext: Option<&str>,
) -> Result<(), FailureDiagnostic> {
    if !response.status().is_success() {
        return Err(FailureDiagnostic::new(
            "photo-cdn",
            user_errors::http_detail(response.status().as_u16()),
        ));
    }
    if let Some(expected) = expected_ext {
        let actual = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .and_then(|value| {
                match value
                    .split(';')
                    .next()?
                    .trim()
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "image/jpeg" => Some("jpg"),
                    "image/png" => Some("png"),
                    "image/webp" => Some("webp"),
                    _ => None,
                }
            });
        if actual != Some(expected) {
            return Err(FailureDiagnostic::new("photo-cdn", "Unsupported format"));
        }
    }
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes)
    {
        return Err(FailureDiagnostic::new("photo-cdn", "Size limit"));
    }
    if response
        .content_length()
        .is_some_and(|size| size > total_remaining)
    {
        return Err(FailureDiagnostic::new(
            "photo-cdn",
            "Temporary storage limit",
        ));
    }
    let target = dir.join(name);
    let temp = tempfile::Builder::new()
        .prefix("photo-")
        .suffix(".part")
        .tempfile_in(dir)
        .map_err(|_| FailureDiagnostic::new("photo-cdn", "File I/O error"))?;
    let mut file = tokio::fs::File::from_std(
        temp.reopen()
            .map_err(|_| FailureDiagnostic::new("photo-cdn", "File I/O error"))?,
    );
    let mut size = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        FailureDiagnostic::new(
            "photo-cdn",
            if error.is_timeout() {
                "Timeout"
            } else {
                "Network error"
            },
        )
    })? {
        size = size.saturating_add(chunk.len() as u64);
        if size > max_bytes {
            return Err(FailureDiagnostic::new("photo-cdn", "Size limit"));
        }
        if size > total_remaining {
            return Err(FailureDiagnostic::new(
                "photo-cdn",
                "Temporary storage limit",
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| FailureDiagnostic::new("photo-cdn", "File I/O error"))?;
    }
    file.flush()
        .await
        .map_err(|_| FailureDiagnostic::new("photo-cdn", "File I/O error"))?;
    drop(file);
    if size == 0 {
        return Err(FailureDiagnostic::new("photo-cdn", "Empty photo"));
    }
    temp.persist_noclobber(&target)
        .map_err(|_| FailureDiagnostic::new("photo-cdn", "File I/O error"))?;
    Ok(())
}

pub(crate) async fn add_images(
    images: &[Url],
    alternates: &[Vec<Url>],
    cfg: &Settings,
    dir: &Path,
    manifest: &Path,
    max_files: usize,
    selection: &mut Selection,
    deadline: time::Instant,
) -> Result<(), Vec<FailureDiagnostic>> {
    let mut failures = Vec::new();
    let mut used = disk_usage(dir);
    for (index, first) in images.iter().enumerate() {
        if selection.files.len() == max_files {
            selection.skipped += images.len() - index;
            break;
        }
        let mut delivered = false;
        let mut failure = None;
        let mut size_rejected = false;
        for url in std::iter::once(first).chain(alternates.get(index).into_iter().flatten()) {
            let path_ext = url.path().rsplit('.').next().unwrap_or("");
            let ext = if url.host_str() == Some("pbs.twimg.com") {
                None
            } else {
                image_extension(path_ext)
            };
            if ext.is_none() && url.host_str() != Some("pbs.twimg.com") {
                failure.get_or_insert(FailureDiagnostic::new("photo-cdn", "Unsupported format"));
                continue;
            }
            let remaining = match &used {
                Ok(used) => cfg
                    .max_media_bytes
                    .saturating_mul(MAX_FILES as u64)
                    .saturating_sub(*used),
                Err(_) => {
                    failure = Some(FailureDiagnostic::new("photo-cdn", "File I/O error"));
                    break;
                }
            };
            if remaining == 0 {
                failure = Some(FailureDiagnostic::new(
                    "photo-cdn",
                    "Temporary storage limit",
                ));
                break;
            }
            let attempt_deadline = if alternates.get(index).is_some_and(|urls| !urls.is_empty()) {
                deadline.min(time::Instant::now() + Duration::from_secs(8))
            } else {
                deadline
            };
            let outcome = time::timeout_at(attempt_deadline, async {
                let client = pinned_client(url)
                    .await
                    .map_err(|_| FailureDiagnostic::new("photo-cdn", "Network error"))?;
                let mut request = client.get(url.clone());
                if url.host_str().is_some_and(|host| {
                    host.ends_with(".tiktokcdn.com")
                        || host.ends_with(".tiktokcdn-us.com")
                        || host.ends_with(".tiktokcdn-eu.com")
                }) {
                    request = request.header(reqwest::header::USER_AGENT, TIKTOK_USER_AGENT);
                }
                let response = request.send().await.map_err(|error| {
                    FailureDiagnostic::new(
                        "photo-cdn",
                        if error.is_timeout() {
                            "Timeout"
                        } else {
                            "Network error"
                        },
                    )
                })?;
                if !response.status().is_success() {
                    return Err(FailureDiagnostic::new(
                        "photo-cdn",
                        user_errors::http_detail(response.status().as_u16()),
                    ));
                }
                let expected_ext = ext;
                let ext = if url.host_str() == Some("pbs.twimg.com") {
                    twitter_image_extension(
                        url,
                        response
                            .headers()
                            .get(reqwest::header::CONTENT_TYPE)
                            .and_then(|header| header.to_str().ok()),
                    )
                    .ok_or_else(|| FailureDiagnostic::new("photo-cdn", "Unsupported format"))?
                } else {
                    ext.expect("other CDN image extension was checked")
                };
                let name = format!("photo-{index:02}.{ext}");
                save_image_response(
                    response,
                    dir,
                    &name,
                    cfg.max_media_bytes,
                    remaining,
                    expected_ext,
                )
                .await?;
                Ok::<_, FailureDiagnostic>((ext, name))
            })
            .await;
            match outcome {
                Ok(Ok((ext, name))) => {
                    let path = dir.join(name);
                    let appended = fs::metadata(&path).and_then(|metadata| {
                        let mut file = fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(manifest)?;
                        let previous_size = file.metadata()?.len();
                        writeln!(file, "{}", path.display())?;
                        Ok(metadata
                            .len()
                            .saturating_add(file.metadata()?.len().saturating_sub(previous_size)))
                    });
                    let added = match appended {
                        Ok(added) => added,
                        Err(_) => {
                            let _ = fs::remove_file(&path);
                            failure = Some(FailureDiagnostic::new("photo-cdn", "File I/O error"));
                            break;
                        }
                    };
                    if let Ok(used) = &mut used {
                        *used = used.saturating_add(added);
                    }
                    selection.files.push(MediaFile {
                        path,
                        kind: if ext == "webp" {
                            MediaKind::Document
                        } else {
                            MediaKind::Photo
                        },
                    });
                    delivered = true;
                    break;
                }
                Ok(Err(diagnostic)) => {
                    size_rejected |= diagnostic.detail == "Size limit";
                    failure = Some(diagnostic);
                }
                Err(_) => {
                    failure = Some(FailureDiagnostic::new("photo-cdn", "Timeout"));
                }
            }
        }
        if !delivered {
            selection.skipped += 1;
            if size_rejected {
                selection.size_skipped += 1;
            }
            if let Some(diagnostic) = failure {
                trace_event(&mut failures, diagnostic);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}
