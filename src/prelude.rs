pub(crate) use std::{
    collections::HashMap,
    env,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[cfg(unix)]
pub(crate) use std::os::unix::process::CommandExt;

pub(crate) use reqwest::redirect::Policy;
pub(crate) use serde_json::Value;
pub(crate) use sqlx::{sqlite::SqliteConnectOptions, sqlite::SqlitePoolOptions, SqlitePool};
pub(crate) use teloxide::{
    prelude::*,
    types::{ChatId, FileId, InputFile, InputMedia, InputMediaAudio, InputMediaDocument,
        InputMediaPhoto, InputMediaVideo, Message, ParseMode, ReplyParameters, UserId},
};
pub(crate) use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::lookup_host, process::Command, sync::{RwLock, Semaphore}, time};
pub(crate) use url::Url;
