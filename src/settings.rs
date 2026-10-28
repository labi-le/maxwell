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
    pub(crate) chat: Option<ChatConfig>,
    pub(crate) chat_locks: Arc<tokio::sync::Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>>,
    pub(crate) self_improve_dir: PathBuf,
    pub(crate) sticker_replies: StickerReplies,
    pub(crate) sticker_catalog: Arc<RwLock<crate::stickers::StickerCatalog>>,
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
        let chat = match env::var("LLM_API_KEY") {
            Ok(key) if !key.trim().is_empty() => {
                let base = env::var("LLM_BASE_URL").unwrap_or_else(|_| DEFAULT_LLM_BASE.to_string());
                let base = parse_llm_url(&base, "LLM_BASE_URL")?;
                let tavily_key = env::var("TAVILY_API_KEY").ok().filter(|value| !value.trim().is_empty());
                let provider = match env::var("SEARCH_PROVIDER").unwrap_or_else(|_| "firecrawl".into()).as_str() {
                    "firecrawl" => SearchProvider::Firecrawl,
                    "tavily" => SearchProvider::Tavily,
                    "ddg" | "duckduckgo" => SearchProvider::DuckDuckGo,
                    _ => return Err("SEARCH_PROVIDER must be firecrawl, tavily, or ddg".into()),
                };
                let search_default = match provider {
                    SearchProvider::Firecrawl => "https://api.firecrawl.dev/v1/search",
                    SearchProvider::Tavily => "https://api.tavily.com/search",
                    SearchProvider::DuckDuckGo => DEFAULT_SEARCH_URL,
                };
                let search = env::var("LLM_SEARCH_URL").unwrap_or_else(|_| search_default.to_string());
                let search = parse_llm_url(&search, "LLM_SEARCH_URL")?;
                let model = env::var("LLM_MODEL").unwrap_or_else(|_| DEFAULT_LLM_MODEL.to_string());
                if model.trim().is_empty() {
                    return Err("LLM_MODEL must not be empty".into());
                }
                Some(ChatConfig::new(base, key, model, search, provider, tavily_key))
            }
            _ => None,
        };
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
            chat,
            chat_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            self_improve_dir: PathBuf::from(env::var("SELF_IMPROVE_DIR").unwrap_or_else(|_| "/var/lib/maksut/self-improve".into())),
            sticker_replies: StickerReplies::default(),
            sticker_catalog: Arc::new(RwLock::new(crate::stickers::StickerCatalog::default())),
            max_media_bytes,
            downloads: Arc::new(Semaphore::new(2)),
            user_rate_limits: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

impl Settings {
    pub(crate) async fn chat_lock(&self, chat_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.chat_locks.lock().await;
        Arc::clone(locks.entry(chat_id).or_default())
    }
}

pub(crate) fn parse_llm_url(raw: &str, name: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| format!("{name} must be a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!("{name} must be an http(s) URL without credentials, query, or fragment"));
    }
    Ok(url)
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
