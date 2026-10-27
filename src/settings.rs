use crate::prelude::*;
use crate::*;

#[derive(Clone)]
pub(crate) struct Settings {
    pub(crate) owner_id: UserId,
    pub(crate) bot_username: String,
    pub(crate) database: SqlitePool,
    pub(crate) download_dir: PathBuf,
    pub(crate) ytdlp_bin: PathBuf,
    pub(crate) python_bin: PathBuf,
    pub(crate) twitter_helper: PathBuf,
    pub(crate) cookies_file: Option<PathBuf>,
    pub(crate) cookie_refresh_url: Option<Url>,
    pub(crate) max_media_bytes: u64,
    pub(crate) downloads: Arc<Semaphore>,
    pub(crate) user_rate_limits: Arc<RwLock<HashMap<UserId, Arc<Semaphore>>>>,
}

impl Settings {
    pub(crate) async fn from_env() -> Result<Self, String> {
        let owner_id = env::var("OWNER_ID")
            .map_err(|_| "OWNER_ID is required".to_string())?
            .parse::<u64>()
            .map_err(|_| "OWNER_ID must be a Telegram user ID".to_string())?;
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:///data/maksut.db".into());
        let database = open_database(&database_url)
            .await
            .map_err(|_| "Unable to open the SQLite database".to_string())?;
        let max_media_bytes = env::var("MAX_MEDIA_BYTES")
            .unwrap_or_else(|_| DEFAULT_MEDIA_LIMIT.to_string())
            .parse::<u64>()
            .map_err(|_| "MAX_MEDIA_BYTES must be an integer in bytes".to_string())?;
        if max_media_bytes == 0 || max_media_bytes > TELEGRAM_FILE_LIMIT {
            return Err("MAX_MEDIA_BYTES must be between 1 and 50000000".into());
        }
        let download_dir = PathBuf::from(
            env::var("DOWNLOAD_DIR").unwrap_or_else(|_| "/tmp/maksut".into()),
        );
        fs::create_dir_all(&download_dir)
            .map_err(|_| "Unable to create DOWNLOAD_DIR".to_string())?;
        let cookies_file = env::var_os("COOKIES_FILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        if let Some(path) = &cookies_file {
            check_cookies(path)?;
        }
        let cookie_refresh_url = env::var("COOKIE_REFRESH_URL")
            .ok()
            .filter(|value| !value.is_empty())
            .map(|value| parse_cookie_refresh_url(&value))
            .transpose()?;
        Ok(Self {
            owner_id: UserId(owner_id),
            bot_username: String::new(),
            database,
            download_dir,
            ytdlp_bin: PathBuf::from(env::var("YTDLP_BIN").unwrap_or_else(|_| "yt-dlp".into())),
            python_bin: PathBuf::from(env::var("PYTHON_BIN").unwrap_or_else(|_| "/opt/yt-dlp/bin/python".into())),
            twitter_helper: PathBuf::from(env::var("TWITTER_HELPER").unwrap_or_else(|_| "/opt/maksut/twitter_images.py".into())),
            cookies_file,
            cookie_refresh_url,
            max_media_bytes,
            downloads: Arc::new(Semaphore::new(2)),
            user_rate_limits: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

pub(crate) fn check_cookies(path: &Path) -> Result<(), String> {
    if !path.is_file() || fs::File::open(path).is_err() {
        return Err("COOKIES_FILE must point to a readable regular file".into());
    }
    Ok(())
}

pub(crate) fn parse_cookie_refresh_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "COOKIE_REFRESH_URL must be a valid URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("COOKIE_REFRESH_URL must be an http(s) URL without credentials, query, or fragment".into());
    }
    Ok(url)
}
