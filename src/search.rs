use crate::prelude::*;
use serde_json::json;

const SEARCH_RESULTS: usize = 5;
const SEARCH_RESULT_CHARS: usize = 2500;
const MAX_SEARCH_BYTES: usize = 512 * 1024;
const SEARCH_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:147.0) Gecko/20100101 Firefox/147.0";

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SearchProvider {
    Firecrawl,
    Tavily,
    DuckDuckGo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) snippet: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchFailure {
    Unavailable,
    Timeout,
    TooLarge,
    Malformed,
    Canceled,
}

impl SearchFailure {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "недоступен",
            Self::Timeout => "таймаут",
            Self::TooLarge => "слишком большой ответ",
            Self::Malformed => "некорректный ответ",
            Self::Canceled => "отменён",
        }
    }
}

/// `Clone` so `JoinSet` tasks can own it instead of borrowing `ChatConfig`.
#[derive(Clone)]
pub(crate) struct SearchClient {
    pub(crate) url: Url,
    pub(crate) provider: SearchProvider,
    pub(crate) tavily_key: Option<String>,
    pub(crate) http: reqwest::Client,
}

impl SearchClient {
    pub(crate) async fn search_hits(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, SearchFailure> {
        match self.provider {
            SearchProvider::Firecrawl => self.firecrawl_search(query, limit).await,
            SearchProvider::Tavily => match self.tavily_key.as_deref() {
                Some(key) => self.tavily_search(key, query, limit).await,
                None => self.firecrawl_search(query, limit).await,
            },
            SearchProvider::DuckDuckGo => self.ddg_search(query, limit).await,
        }
    }

    fn map_error(error: reqwest::Error) -> SearchFailure {
        if error.is_timeout() {
            SearchFailure::Timeout
        } else {
            SearchFailure::Unavailable
        }
    }

    async fn response_bytes(response: reqwest::Response) -> Result<Vec<u8>, SearchFailure> {
        if response
            .content_length()
            .is_some_and(|length| length > MAX_SEARCH_BYTES as u64)
        {
            return Err(SearchFailure::TooLarge);
        }
        let bytes = response.bytes().await.map_err(Self::map_error)?;
        if bytes.len() > MAX_SEARCH_BYTES {
            return Err(SearchFailure::TooLarge);
        }
        Ok(bytes.to_vec())
    }

    async fn tavily_search(
        &self,
        key: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, SearchFailure> {
        let body = json!({
            "api_key": key,
            "query": query,
            "max_results": limit,
            "search_depth": "basic",
        });
        let response = self
            .http
            .post(self.url.clone())
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(Self::map_error)?;
        if !response.status().is_success() {
            return Err(SearchFailure::Unavailable);
        }
        let bytes = Self::response_bytes(response).await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| SearchFailure::Malformed)?;
        if value
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| !error.trim().is_empty())
        {
            return Err(SearchFailure::Unavailable);
        }
        Ok(value
            .get("results")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .take(limit)
            .map(|item| SearchHit {
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                url: item
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                snippet: item
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            })
            .collect())
    }

    async fn firecrawl_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, SearchFailure> {
        let body = json!({"query": query, "limit": limit});
        let response = self
            .http
            .post(self.url.clone())
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(Self::map_error)?;
        if !response.status().is_success() {
            return Err(SearchFailure::Unavailable);
        }
        let bytes = Self::response_bytes(response).await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| SearchFailure::Malformed)?;
        if value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(SearchFailure::Unavailable);
        }
        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .take(limit)
            .map(|item| SearchHit {
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                url: item
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                snippet: item
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            })
            .collect())
    }

    async fn ddg_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchFailure> {
        let mut url = self.url.clone();
        url.query_pairs_mut().append_pair("q", query);
        let response = self
            .http
            .get(url)
            .header("user-agent", SEARCH_USER_AGENT)
            .send()
            .await
            .map_err(Self::map_error)?;
        if !response.status().is_success() {
            return Err(SearchFailure::Unavailable);
        }
        let bytes = Self::response_bytes(response).await?;
        let html = String::from_utf8_lossy(&bytes);
        Ok(extract_search_results(&html, limit)
            .into_iter()
            .map(|(title, url, snippet)| SearchHit {
                title,
                url,
                snippet,
            })
            .collect())
    }

    pub(crate) async fn web_search(&self, query: &str) -> String {
        let hits = match self.search_hits(query, SEARCH_RESULTS).await {
            Ok(hits) => hits,
            Err(SearchFailure::TooLarge) => return "Поиск вернул слишком большой ответ.".into(),
            Err(_) => return "Поиск недоступен.".into(),
        };
        if hits.is_empty() {
            return "Ничего не найдено.".into();
        }
        let mut report = String::new();
        for (index, hit) in hits.iter().enumerate() {
            report.push_str(&format!(
                "{}. {}\n{}\n{}\n\n",
                index + 1,
                hit.title,
                hit.url,
                hit.snippet
            ));
            if report.chars().count() > SEARCH_RESULT_CHARS {
                break;
            }
        }
        report.chars().take(SEARCH_RESULT_CHARS).collect()
    }
}

pub(crate) fn extract_search_results(html: &str, limit: usize) -> Vec<(String, String, String)> {
    let mut results = Vec::new();
    let mut cursor = 0;
    while results.len() < limit {
        let Some(relative) = html[cursor..].find("class=\"result__a\"") else {
            break;
        };
        let marker = cursor + relative;
        let Some(tag_relative) = html[..marker].rfind("<a ") else {
            break;
        };
        let tag_end = html[marker..]
            .find('>')
            .map(|index| marker + index + 1)
            .unwrap_or(html.len());
        let tag = &html[tag_relative..tag_end];
        let href = tag
            .find("href=\"")
            .and_then(|start| {
                tag[start + 6..]
                    .find('"')
                    .map(|end| &tag[start + 6..start + 6 + end])
            })
            .map(decode_link)
            .unwrap_or_default();
        let title_end = html[tag_end..]
            .find("</a>")
            .map(|index| tag_end + index)
            .unwrap_or(html.len());
        let title = clean_text(&html[tag_end..title_end]);
        let next_result = html[tag_end..]
            .find("class=\"result__a\"")
            .map(|index| tag_end + index)
            .unwrap_or(html.len());
        let snippet = html[title_end..next_result]
            .find("class=\"result__snippet\"")
            .and_then(|index| {
                let marker = title_end + index;
                let text_start = html[marker..].find('>').map(|offset| marker + offset + 1)?;
                let text_end = html[text_start..next_result]
                    .find("</a>")
                    .map(|offset| text_start + offset)?;
                (text_start < text_end).then(|| clean_text(&html[text_start..text_end]))
            })
            .unwrap_or_default();
        if !href.is_empty() {
            results.push((title, href, snippet));
        }
        cursor = tag_end.max(marker + 1);
    }
    results
}

fn decode_link(raw: &str) -> String {
    let href = raw.replace("&amp;", "&");
    if let Some((_, query)) = href.split_once("uddg=") {
        let encoded = query.split('&').next().unwrap_or(query);
        let decoded: String = url::form_urlencoded::parse(format!("u={encoded}").as_bytes())
            .map(|(_, value)| value.into_owned())
            .collect();
        if decoded.starts_with("http://") || decoded.starts_with("https://") {
            return decoded;
        }
    }
    href.strip_prefix("//")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or(href)
}

pub(crate) fn clean_text(raw: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    let text = decode_entities(&text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find('&') {
        output.push_str(&rest[..position]);
        let after = &rest[position..];
        let entity_end = after[1..]
            .find(';')
            .filter(|offset| *offset <= 32)
            .map(|offset| offset + 1);
        let Some(end) = entity_end else {
            output.push('&');
            rest = &after[1..];
            continue;
        };
        match decode_entity(&after[1..end]) {
            Some(ch) => {
                output.push(ch);
                rest = &after[end + 1..];
            }
            None => {
                output.push('&');
                rest = &after[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

fn decode_entity(entity: &str) -> Option<char> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .filter(|ch| !ch.is_control());
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return decimal
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .filter(|ch| !ch.is_control());
    }
    let named = match entity.to_ascii_lowercase().as_str() {
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        "lt" => '<',
        "gt" => '>',
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "times" => '×',
        "divide" => '÷',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "deg" => '°',
        "middot" => '·',
        "bull" => '•',
        "dagger" => '†',
        "prime" => '′',
        "plusmn" => '±',
        "micro" => 'µ',
        "para" => '¶',
        "sect" => '§',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "sup2" => '²',
        "sup3" => '³',
        "szlig" => 'ß',
        "shy" => '\u{00AD}',
        "zwnj" => '\u{200C}',
        "zwj" => '\u{200D}',
        "lrm" => '\u{200E}',
        "rlm" => '\u{200F}',
        _ => return None,
    };
    Some(named)
}
