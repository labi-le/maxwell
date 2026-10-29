# Chrome cookies and the refresh sidecar

> **When to read this:** Read before touching `src/cookies.rs`, `src/cookie_refresh.rs`, `tools/export_chrome_cookies.py`, `tools/cookie_refresh_service.py`, `cookie_refresh/Dockerfile`, or the `cookie-refresh` service in `compose.yaml`.

## How cookie export and refresh work ([ADR 008](../adr/008-docker-cookie-refresh-service.md))

Cookies come from an authenticated Chrome profile (`~/.config/google-chrome/Default/Cookies`) on `notebook` or `pc`. Because Chrome stores its decryption key in the host keyring, cookie decryption runs over SSH on the target host itself rather than inside the bot container.

- `tools/export_chrome_cookies.py` runs on `pc` or `notebook`, decrypts `v10`/`v11` cookies, filters to supported platform domains, writes a Netscape cookie jar to stdout, and writes a redacted summary to stderr.
- The `cookie-refresh` sidecar (`cookie_refresh/Dockerfile`, `tools/cookie_refresh_service.py`) listens internally on `:8080` (`GET /healthz`, `POST /refresh` with `{"source":"youtube|tiktok|instagram|twitter|pinterest|generic"}`). Its port is not published on the host.
- On `POST /refresh`, the sidecar queries `COOKIE_SSH_TARGETS` (`labile@pc,labile@notebook`, mapped via `extra_hosts` to `COOKIE_HOST_PC=192.168.1.3` and `COOKIE_HOST_NOTEBOOK=192.168.1.155`), selects the freshest Chrome profile that contains cookies for the requested source, and atomically replaces `/run/secrets/maksut/cookies.txt` with mode `0600`. If no profile qualifies, it returns `503` and leaves the existing file untouched.
- The host directory `${MAKSUT_COOKIES_DIR:-/home/eden/.local/share/maksut/cookies}` is bind-mounted `:rw` into `cookie-refresh` and `:ro` into `maksut` (as a directory mount so atomic file replacement stays visible without restarting `maksut`).

## When the bot triggers a refresh (`src/cookie_refresh.rs`, `src/download.rs`)

- Triggered **only** on an explicit authentication failure, expired-cookie message (`cookies are no longer valid` when the download failed), or missing/unusable cookie jar.
- Ordinary HTTP `403`, private/deleted posts, or missing metadata never trigger a refresh. A `cookies are no longer valid` warning on a download that succeeded does not trigger a retry ([ADR 007](../adr/007-telegram-timeout-and-cookie-warnings.md)).
- Refresh responses are deduplicated for 5 seconds so parallel video + X photo extraction on one link does not run two SSH exports.
- After a successful refresh, the bot retries the failed operation **at most once**:
  - Video and metadata downloads retry only when no completed files were already produced.
  - For X/Twitter, if the video already succeeded and only the photo helper failed with an auth error, only the photo helper is retried.
  - For TikTok and Pinterest/Instagram photo posts, cookies are refreshed before re-reading post metadata.

## Initial SSH key setup and manual fallback

Before first start, create a dedicated service SSH key in `.secrets/cookie-refresh` (gitignored and excluded from the Docker build context):

```sh
install -d -m 700 .secrets/cookie-refresh
ssh-keygen -t ed25519 -N '' -f .secrets/cookie-refresh/id_ed25519
ssh-copy-id -i .secrets/cookie-refresh/id_ed25519.pub labile@pc
ssh-copy-id -i .secrets/cookie-refresh/id_ed25519.pub labile@notebook
ssh-keyscan -H pc notebook > .secrets/cookie-refresh/known_hosts
chmod 600 .secrets/cookie-refresh/id_ed25519 .secrets/cookie-refresh/known_hosts
```

Verify the `ssh-keyscan` fingerprints against the hosts' known fingerprints. You can restrict the key in `~/.ssh/authorized_keys` on each host with `command="python3 -",restrict` so the sidecar can only feed `export_chrome_cookies.py` on stdin.

If the sidecar is temporarily unavailable, use the `chrome-cookie-session` skill to export cookies manually into a temporary file, validate it, and install it with `install -m 600`. Never redirect exporter stdout straight into the live `cookies.txt`.
