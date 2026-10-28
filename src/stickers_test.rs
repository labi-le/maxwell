use crate::*;

#[test]
fn sticker_rule_matches_each_trigger_in_any_case() {
    assert_eq!(sticker_rule("да"), Some(PIZDA));
    assert_eq!(sticker_rule("дА!"), Some(PIZDA));
    assert_eq!(sticker_rule("да, конечно"), Some(PIZDA));
    assert_eq!(sticker_rule("да это точно"), Some(PIZDA));
    assert_eq!(sticker_rule("НЕТ"), Some(PIDORA_OTVET));
    assert_eq!(sticker_rule("нет, спасибо"), Some(PIDORA_OTVET));
    assert_eq!(sticker_rule("ну"), Some(BARANKI_GNU));
    assert_eq!(sticker_rule("Ну!"), Some(BARANKI_GNU));
    assert_eq!(sticker_rule("ну да"), Some(PIZDA));
    assert_eq!(sticker_rule("ну, 300 рублей"), Some(TRACTORIST));
    assert_eq!(sticker_rule("300"), Some(TRACTORIST));
    assert_eq!(sticker_rule("300$"), Some(TRACTORIST));
    assert_eq!(sticker_rule("300 баксов"), Some(TRACTORIST));
    assert_eq!(sticker_rule("триста рублей"), Some(TRACTORIST));
    assert_eq!(sticker_rule("Молодец"), Some(MOLODCY));
    assert_eq!(sticker_rule("его назвали молодцом"), Some(MOLODCY));
    assert_eq!(sticker_rule("грусть-тоска"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("мне грустно"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("я грустный"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("загрустил"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("у меня депрессия"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("депрешн"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("депресняк"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("печально"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("тоскливо на душе"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("хандра"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("расстроился"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("подавленное настроение"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("на душе кошки скребут"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("нет сил вообще"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("настроение на нуле"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("всё валится из рук"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("хочется плакать"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("всё тлен"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("паршиво"), Some(GRUSTNO_VEZDE));
    assert_eq!(sticker_rule("мне одиноко"), Some(GRUSTNO_VEZDE));
}

#[test]
fn sticker_rule_ignores_substrings_and_other_numbers() {
    for text in ["", "нету", "дата", "данет", "прода", "интернет", "еда", "Даниил", "медаль",
        "1300", "3000", "300000", "тристаа", "печатать", "тосканский", "молодежный", "123",
        "🙂", "смеюсь до слёз", "я плачу от счастья", "нетсилы", "тленный",
        "сначала нет, потом да", "да это точно так и есть", "нет, я не пойду сегодня",
        "ты сказал да вчера вечером", "я не знаю, но наверное да",
        "ну и что теперь делать", "ну это понятно, а дальше", "Ну, погоди"]
    {
        assert_eq!(sticker_rule(text), None, "{text:?} must not trigger a sticker");
    }
}

#[test]
fn sticker_rule_takes_the_first_trigger_in_the_message() {
    assert_eq!(sticker_rule("да нет"), Some(PIZDA));
    assert_eq!(sticker_rule("ну 300"), Some(TRACTORIST));
}
