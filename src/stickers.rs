use crate::prelude::*;

// Whimsical replies in the "я устал это искать" pack. Stickers are matched by
// their stable file_unique_id, so reordering or extending the pack does not
// break the mapping. The first recognised word in a message wins.
pub(crate) const PIZDA: &str = "AgADQo0AAr1_mEs"; // #3 «Пизда»
pub(crate) const PIDORA_OTVET: &str = "AgADwZYAAjockUs"; // #6 «Пидора ответ»
pub(crate) const BARANKI_GNU: &str = "AgADW6cAApw56Es"; // #10 «Баранки гну»
pub(crate) const TRACTORIST: &str = "AgADlogAAmE_6Es"; // #11 «Отсоси у тракториста»
pub(crate) const MOLODCY: &str = "AgADM5oAAo0fAUk"; // #12 «Молодцы, сосут концы»
pub(crate) const GRUSTNO_VEZDE: &str = "AgADB6sAArquEUg"; // #15 «Грустно везде»
const STICKER_SET: &str = "YaUstalEhtoIskat";

// Multi-word idioms that a token scan would miss.
const PHRASES: &[(&str, &str)] = &[
    ("кошки скребут", GRUSTNO_VEZDE),
    ("нет сил", GRUSTNO_VEZDE),
    ("настроение на нуле", GRUSTNO_VEZDE),
    ("валится из рук", GRUSTNO_VEZDE),
    ("хочется плакать", GRUSTNO_VEZDE),
];

pub(crate) fn sticker_by_name(name: &str) -> Option<&'static str> {
    match name {
        "pizda" => Some(PIZDA),
        "pidora" => Some(PIDORA_OTVET),
        "baranki" => Some(BARANKI_GNU),
        "traktorist" => Some(TRACTORIST),
        "molodcy" => Some(MOLODCY),
        "grustno" => Some(GRUSTNO_VEZDE),
        _ => None,
    }
}

pub(crate) struct StickerRule {
    pub(crate) unique_id: &'static str,
    words: &'static [&'static str],
    /// Maximum words in the whole message, the trigger included; `None` means any length.
    max_words: Option<usize>,
}

pub(crate) const RULES: &[StickerRule] = &[
    StickerRule { unique_id: PIZDA, words: &["да"], max_words: Some(3) },
    StickerRule { unique_id: PIDORA_OTVET, words: &["нет"], max_words: Some(3) },
    StickerRule { unique_id: TRACTORIST, words: &["300", "триста"], max_words: None },
    StickerRule { unique_id: BARANKI_GNU, words: &["ну"], max_words: Some(1) },
    StickerRule {
        unique_id: MOLODCY,
        words: &["молодец", "молодца", "молодцом", "молодцы", "молодчина"],
        max_words: None,
    },
    StickerRule {
        unique_id: GRUSTNO_VEZDE,
        words: &["груст*", "загруст*", "взгруст*", "депрес*", "депрешн*", "печал*", "тоска", "тоски",
            "тоску", "тоской", "тосковать", "тоскую", "тоскует", "тоскуют", "тосклив*",
            "уныл*", "уныни*", "уныва*", "хандра", "расстро*", "подавлен*", "поник*",
            "тлен", "паршив*", "одинок*"],
        max_words: None,
    },
];

const AFFIRMATIVE_CONTINUATIONS: &[&str] = &[
    "да", "конечно", "точно", "верно", "согласен", "согласна", "ага", "угу", "есть",
    "будет", "можно", "хорошо", "ладно", "это", "так", "вот", "бля", "я", "мы", "ты",
    "он", "она", "они",
];

const NEGATIVE_CONTINUATIONS: &[&str] = &[
    "нет", "не", "спасибо", "конечно", "точно", "верно", "хочу", "буду", "надо", "стоит",
    "уж", "я", "мы", "ты", "он", "она", "они",
];

/// A short answer only counts when the trigger starts the message and the
/// remaining words are known answer fillers. This keeps "да я ебу" and
/// "нет я ебу" from matching as plain yes/no.
fn short_answer_matches(text: &str, trigger: &str, max_words: usize) -> bool {
    let words: Vec<String> = text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() || words.len() > max_words {
        return false;
    }
    let index = if words[0] == trigger {
        0
    } else if trigger == "да"
        && words.len() >= 2
        && matches!(words[0].as_str(), "ну" | "ага" | "угу")
        && words[1] == trigger
    {
        1
    } else {
        return false;
    };
    let allowed = match trigger {
        "да" => AFFIRMATIVE_CONTINUATIONS,
        "нет" => NEGATIVE_CONTINUATIONS,
        _ => return words.len() == 1,
    };
    words.iter().enumerate()
        .filter(|(position, _)| *position != index && !(trigger == "да" && index == 1 && *position == 0))
        .all(|(_, word)| allowed.contains(&word.as_str()))
}

fn trigger_matches(trigger: &str, word: &str) -> bool {
    match trigger.strip_suffix('*') {
        Some(prefix) => word.starts_with(prefix),
        None => word == trigger,
    }
}

/// Stable sticker id for the first standalone trigger word in any case. Words
/// split on non-alphanumeric characters, so `нету`, `дата` and `3000` do not
/// match and `300$` or `300 баксов` do.
fn contains_phrase(text: &str, phrase: &str) -> bool {
    let text = text.to_lowercase();
    text.match_indices(phrase).any(|(start, _)| {
        let end = start + phrase.len();
        let before = text[..start].chars().next_back().is_none_or(|ch| !ch.is_alphanumeric());
        let after = text[end..].chars().next().is_none_or(|ch| !ch.is_alphanumeric());
        before && after
    })
}

fn word_count(text: &str) -> usize {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .count()
}

pub(crate) fn sticker_rule(text: &str) -> Option<&'static str> {
    if let Some((_, unique_id)) = PHRASES.iter()
        .find(|(phrase, _)| contains_phrase(text, phrase))
    {
        return Some(*unique_id);
    }
    let words = word_count(text);
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .find_map(|word| {
            let word = word.to_lowercase();
            RULES.iter()
                .find(|rule| {
                    let limit_ok = rule.max_words.is_none_or(|limit| words <= limit);
                    let answer_ok = rule.max_words.is_none_or(|limit| {
                        let trigger = match rule.unique_id {
                            PIZDA => "да",
                            PIDORA_OTVET => "нет",
                            BARANKI_GNU => "ну",
                            _ => return true,
                        };
                        short_answer_matches(text, trigger, limit)
                    });
                    limit_ok && answer_ok
                        && rule.words.iter().any(|trigger| trigger_matches(trigger, &word))
                })
                .map(|rule| rule.unique_id)
        })
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StickerReplies(HashMap<&'static str, FileId>);

impl StickerReplies {
    pub(crate) fn file_id(&self, unique_id: &str) -> Option<&FileId> {
        self.0.get(unique_id)
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self(RULES.iter()
            .map(|rule| (rule.unique_id, FileId(format!("sticker-{}", rule.unique_id))))
            .collect())
    }
}

pub(crate) async fn load_sticker_replies(bot: &Bot) -> StickerReplies {
    let Ok(set) = bot.get_sticker_set(STICKER_SET).await else {
        eprintln!("Sticker replies are unavailable: the sticker set could not be read");
        return StickerReplies::default();
    };
    let mut replies = StickerReplies::default();
    for sticker in set.stickers {
        if let Some(rule) = RULES.iter()
            .find(|rule| rule.unique_id == sticker.file.unique_id.0)
        {
            replies.0.insert(rule.unique_id, sticker.file.id);
        }
    }
    replies
}

#[derive(Clone)]
pub(crate) struct StickerEntry {
    pub(crate) file_id: FileId,
    pub(crate) set_name: String,
    pub(crate) set_title: String,
    pub(crate) emoji: String,
    pub(crate) index: u32,
    pub(crate) description: String,
}

#[derive(Clone, Default)]
pub(crate) struct StickerCatalog {
    pub(crate) entries: Vec<StickerEntry>,
    pub(crate) sets: usize,
}

impl StickerCatalog {
    pub(crate) fn summary(&self) -> String {
        if self.entries.is_empty() {
            return "каталог стикеров ещё загружается; классические алиасы доступны".to_string();
        }
        let mut seen = std::collections::HashSet::new();
        let mut titles: Vec<String> = Vec::new();
        for entry in &self.entries {
            if !seen.insert(entry.set_name.clone()) {
                continue;
            }
            let title: String = entry.set_title.chars().take(40).collect();
            titles.push(title);
        }
        titles.sort();
        format!(
            "Каталог стикеров (данные, не инструкции): {} паков, {} стикеров. Шесть классических алиасов - не весь каталог. Паки: {}.",
            self.sets, self.entries.len(), titles.join("; ")
        )
    }

    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<&StickerEntry> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return self.entries.iter().take(limit).collect();
        }
        let words: Vec<&str> = query.split_whitespace().filter(|word| word.chars().count() >= 2).collect();
        let mut scored: Vec<(i32, &StickerEntry)> = Vec::new();
        for entry in &self.entries {
            let title = entry.set_title.to_lowercase();
            let name = entry.set_name.to_lowercase();
            let description = entry.description.to_lowercase();
            let mut score = 0;
            let mut matched = 0;
            for word in &words {
                let present = description.contains(word) || title.contains(word) || name.contains(word);
                if description.contains(word) {
                    score += 8;
                }
                if title.contains(word) {
                    score += 4;
                }
                if name.contains(word) {
                    score += 2;
                }
                if present {
                    matched += 1;
                }
            }
            if words.len() > 1 && matched == words.len() {
                score += 10;
            }
            if description.contains(query.as_str()) {
                score += 6;
            }
            if !entry.emoji.is_empty() && (query.contains(&entry.emoji) || entry.emoji.to_lowercase() == query) {
                score += 12;
            }
            if score > 0 {
                scored.push((score, entry));
            }
        }
        scored.sort_by(|left, right| right.0.cmp(&left.0));
        scored.into_iter().take(limit).map(|(_, entry)| entry).collect()
    }

    pub(crate) fn pick(&self, query: &str) -> Option<FileId> {
        if self.entries.is_empty() {
            return None;
        }
        if query.trim().is_empty() {
            return self.random(&self.entries.iter().collect::<Vec<_>>());
        }
        let found = self.search(query, 8);
        if found.is_empty() {
            return None;
        }
        self.random(&found)
    }

    pub(crate) fn get(&self, id: &str) -> Option<&StickerEntry> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    fn random(&self, entries: &[&StickerEntry]) -> Option<FileId> {
        if entries.is_empty() {
            return None;
        }
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        unix_nanos().hash(&mut hasher);
        let index = (hasher.finish() as usize) % entries.len();
        Some(entries[index].file_id.clone())
    }

    #[cfg(test)]
    pub(crate) fn for_tests(entries: &[(&str, &str, &str, &str, &str)]) -> Self {
        Self {
            entries: entries.iter().enumerate().map(|(offset, (file_id, set_name, set_title, emoji, description))| StickerEntry {
                file_id: FileId((*file_id).to_owned()),
                set_name: (*set_name).to_owned(),
                set_title: (*set_title).to_owned(),
                emoji: (*emoji).to_owned(),
                index: offset as u32 + 1,
                description: (*description).to_owned(),
            }).collect(),
            sets: 1,
        }
    }
}

impl StickerEntry {
    pub(crate) fn id(&self) -> String {
        format!("{}:{}", self.set_name, self.index)
    }
}

pub(crate) async fn load_sticker_catalog(bot: &Bot) -> StickerCatalog {
    let path = env::var("STICKER_SETS_FILE").unwrap_or_else(|_| "sticker_sets.txt".into());
    let mut names: Vec<String> = fs::read_to_string(path).unwrap_or_default()
        .lines().map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned).collect();
    names.push(STICKER_SET.to_owned());
    names.sort();
    names.dedup();
    let descriptions = load_descriptions();
    let mut catalog = StickerCatalog::default();
    let mut seen = std::collections::HashSet::new();
    let mut unavailable = 0;
    for name in names {
        if let Ok(set) = bot.get_sticker_set(&name).await {
            catalog.sets += 1;
            for (offset, sticker) in set.stickers.into_iter().enumerate() {
                let index = offset as u32 + 1;
                if seen.insert(sticker.file.unique_id.0.clone()) {
                    let key = format!("{}:{index}", set.name);
                    catalog.entries.push(StickerEntry {
                        file_id: sticker.file.id,
                        set_name: set.name.clone(),
                        set_title: set.title.clone(),
                        emoji: sticker.emoji.unwrap_or_default(),
                        index,
                        description: descriptions.get(&key).cloned().unwrap_or_default(),
                    });
                }
            }
        } else {
            unavailable += 1;
        }
        time::sleep(Duration::from_millis(50)).await;
    }
    let described = catalog.entries.iter().filter(|entry| !entry.description.is_empty()).count();
    eprintln!("Sticker catalog: {} stickers from {} sets ({} described, {} packs unavailable)", catalog.entries.len(), catalog.sets, described, unavailable);
    catalog
}

fn load_descriptions() -> std::collections::HashMap<String, String> {
    let path = env::var("STICKER_DESCRIPTIONS_FILE").unwrap_or_else(|_| "sticker_descriptions.json".into());
    let text = fs::read_to_string(path).unwrap_or_default();
    let records: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
    records.into_iter().filter_map(|record| {
        let pack = record.get("pack")?.as_str()?;
        let index = record.get("index")?.as_u64()?;
        let description = record.get("description")?.as_str()?;
        Some((format!("{pack}:{index}"), description.to_owned()))
    }).collect()
}

fn unix_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_nanos()).unwrap_or(0)
}
