mod prelude;
use prelude::*;

mod media_cache;
mod user_errors;

pub(crate) use media_cache::{cache_key, init_cache, invalidate_cache, lookup_cache, store_cache, CachedMedia};

mod settings;
pub(crate) use settings::*;
mod cookies;
pub(crate) use cookies::*;
mod database;
pub(crate) use database::*;
mod diagnostics;
pub(crate) use diagnostics::*;
mod links;
pub(crate) use links::*;
mod media;
pub(crate) use media::*;
mod process;
pub(crate) use process::*;
mod images;
pub(crate) use images::*;
mod image_fetch;
pub(crate) use image_fetch::*;
mod youtube;
pub(crate) use youtube::*;
mod download;
pub(crate) use download::*;
mod cookie_refresh;
pub(crate) use cookie_refresh::*;
mod download_videos;
pub(crate) use download_videos::*;
mod caption;
pub(crate) use caption::*;
mod video_convert;
pub(crate) use video_convert::*;
mod delivery;
pub(crate) use delivery::*;
mod source_message;
pub(crate) use source_message::*;
mod handler;
pub(crate) use handler::*;
mod cleanup;
pub(crate) use cleanup::*;

const MAX_FILES: usize = 10;
// TikTok photo posts can carry up to 35 images; Telegram albums are split at 10.
const MAX_TIKTOK_PHOTOS: usize = 35;
// Three pending or active downloads allow short bursts without one user filling the queue.
const USER_DOWNLOAD_LIMIT: usize = 3;
const TELEGRAM_FILE_LIMIT: u64 = 50_000_000;
const DEFAULT_MEDIA_LIMIT: u64 = 45 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const TELEGRAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const COOKIE_REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
const YOUTUBE_PROBE_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_METADATA_BYTES: u64 = 4 * 1024 * 1024;
// Reserve room for MP4 muxing; a stream-size estimate alone is not a sendable file size.
const MP4_MUX_HEADROOM: u64 = 64 * 1024;
const MAX_TWITTER_JSON_BYTES: u64 = 16 * 1024;
const MAX_VIDEO_DIMENSION: u32 = 3840;
const MAX_VIDEO_PIXELS: u32 = 3840 * 2160;
const TIKTOK_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:147.0) Gecko/20100101 Firefox/147.0";


fn telegram_bot(token: String) -> Result<Bot, reqwest::Error> {
    let client = teloxide::net::default_reqwest_settings()
        .timeout(TELEGRAM_REQUEST_TIMEOUT)
        .build()?;
    Ok(Bot::with_client(token, client))
}

#[tokio::main]
async fn main() {
    let token = match env::var("BOT_TOKEN") {
        Ok(token) if !token.trim().is_empty() => token,
        _ => {
            eprintln!("BOT_TOKEN is required");
            std::process::exit(1);
        }
    };
    let mut cfg = match Settings::from_env().await {
        Ok(cfg) => cfg,
        Err(reason) => {
            eprintln!("{reason}");
            std::process::exit(1);
        }
    };
    cleanup_old_downloads(&cfg.download_dir);
    let bot = match telegram_bot(token) {
        Ok(bot) => bot,
        Err(_) => {
            eprintln!("Unable to initialize Telegram HTTP client");
            std::process::exit(1);
        }
    };
    cfg.bot_username = match bot.get_me().await {
        Ok(me) => me.user.username.unwrap_or_default(),
        Err(error) => {
            eprintln!("Unable to identify bot: {error}");
            std::process::exit(1);
        }
    };
    let cfg = Arc::new(cfg);
    teloxide::repl(bot, move |bot: Bot, message: Message| {
        let cfg = Arc::clone(&cfg);
        async move { handle_message(bot, message, cfg).await }
    })
    .await;
}


#[cfg(test)]
mod test_support;
#[cfg(test)]
mod feature_support;
#[cfg(test)]
mod cookies_test;
#[cfg(test)]
mod links_test;
#[cfg(test)]
mod images_test;
#[cfg(test)]
mod image_fetch_test;
#[cfg(test)]
mod youtube_test;
#[cfg(test)]
mod caption_test;
#[cfg(test)]
mod video_convert_test;
#[cfg(test)]
mod media_test;
#[cfg(test)]
mod process_test;
#[cfg(test)]
mod cookie_refresh_test;
#[cfg(test)]
mod download_test;
#[cfg(test)]
mod diagnostics_test;
#[cfg(test)]
mod user_errors_test;
#[cfg(test)]
mod database_test;
#[cfg(test)]
mod handler_test;
#[cfg(test)]
mod delivery_test;
#[cfg(test)]
mod media_cache_test;
#[cfg(test)]
mod download_videos_test;
