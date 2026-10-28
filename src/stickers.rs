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
                .find(|rule| rule.max_words.is_none_or(|limit| words <= limit)
                    && rule.words.iter().any(|trigger| trigger_matches(trigger, &word)))
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
