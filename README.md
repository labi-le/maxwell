# Максут

Telegram-бот на Rust (`maksut`) для скачивания медиа по ссылкам YouTube, Pinterest, X/Twitter, Instagram и TikTok, свободного общения в чате и самоулучшения через изолированный пайплайн.

Архитектура, ограничения и детали реализации вынесены в [`AGENTS.md`](./AGENTS.md) и тематические гайды в [`docs/guides/`](./docs/guides/):

- [`docs/guides/layout.md`](./docs/guides/layout.md): карта модулей `src/` и Python-сервисов `tools/`
- [`docs/guides/media.md`](./docs/guides/media.md): загрузка видео и фото, проверка аудио, конвертация в MP4, альбомы, подписи, 24-часовой кэш `file_id` и категории ошибок
- [`docs/guides/chat.md`](./docs/guides/chat.md): свободное общение с LLM, память чата, стикеры, `web_search`, `research`, `read_url` и реестр инструментов
- [`docs/guides/self-improve.md`](./docs/guides/self-improve.md): двухэтапный пайплайн самоулучшения (`plan` -> `deploy`) и контракт `tools/omp_worker.py`
- [`docs/guides/cookies.md`](./docs/guides/cookies.md): экспорт cookies Chrome по SSH и сервис `cookie-refresh`
- [`docs/guides/security.md`](./docs/guides/security.md): SSRF-фильтры, границы прав инструментов, защита от промпт-инъекций и изоляция контейнеров
- [`docs/guides/workflow.md`](./docs/guides/workflow.md): обязательный цикл разработки и ревью

## Запуск и обслуживание

Бот работает через исходящие соединения, без `host network`, опубликованных портов и внешней СУБД. Для первого запуска создайте `.env` из `.env.example` (`chmod 600 .env`), задайте `BOT_TOKEN` и `OWNER_ID`, настройте SSH-ключ `cookie-refresh` по [`docs/guides/cookies.md`](./docs/guides/cookies.md) и создайте каталог `self-improve/`:

```sh
mkdir -p self-improve
docker compose -f compose.yaml up --build -d
docker compose -f compose.yaml logs -f maksut
```

Обновление и остановка (не используйте `docker compose down -v`, чтобы не удалить volume `maksut_data` с SQLite-базой):

```sh
docker compose -f compose.yaml up --build -d
docker compose -f compose.yaml down
```

Для смены `BOT_TOKEN`, `OWNER_ID` или путей в `.env` пересоздайте контейнер через `docker compose -f compose.yaml up -d --force-recreate` (`env_file` читает переменные из `.env`, экспортированные в оболочке значения их не переопределяют). Обычное обновление `cookies.txt` перезапуска не требует.

Запуск хостового оркестратора самоулучшения:

```sh
cp packaging/maksut-self-improve.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now maksut-self-improve.service
journalctl --user -u maksut-self-improve.service -f
```

Для локальной разработки используется `shell.nix`:

```sh
CARGO_TARGET_DIR=/tmp/maxwell-target nix-shell --run "cargo test"
nix-shell --run "python3 -m unittest discover -s tools -p 'test_*.py'"
```

## Использование

- **Ссылки и группы:** в личном чате бот принимает ссылки только от владельца (`OWNER_ID`). В группах обработка по умолчанию выключена и управляется владельцем через `/maksut on`, `/maksut off` и `/maksut status` (у бота должен быть выключен Privacy Mode в BotFather и выдано право `can_delete_messages` для удаления исходной ссылки после доставки). Во включённой группе ссылки принимает от любого участника с лимитом на отправителя (`USER_DOWNLOAD_LIMIT = 3`); остальные действия остаются owner-only.
- **Свободное общение (`LLM_API_KEY`):** в личке отвечает владельцу на любой текст без ссылки и команды; во включённой группе отвечает реплаем на `@имя_бота`, реплай на сообщение бота, обращение `максут`/`maksut`, а также поддерживает живой диалог с собеседником ещё 75 секунд после своего ответа без повторного упоминания имени (если реплика не по теме — молчит). Всем участникам доступны `web_search`, `research`, `read_url`, `propose_feature` и `feature_status`; владельцу дополнительно доступны `feature_confirmation`, `inspect_code`, `find_stickers` и `send_sticker` ([`docs/guides/chat.md`](./docs/guides/chat.md)).
- **Самоулучшение:** владелец может ставить задачи через `/improve <цель>` или в диалоге (`propose_feature`), проверять очередь (`/improve status [id]`, `feature_status`, `feature_confirmation`) и управлять заявками кнопками либо командами в личке: `одобри #id`, `отклони #id`, `отмени #id`, `подними #id`, `опусти #id`, `приоритет #id <n>`, `/improve off` ([`docs/guides/self-improve.md`](./docs/guides/self-improve.md)).

## Диагностика неудачных загрузок

Безопасная трасса последних 200 ошибок загрузки хранится в таблицах `download_failures` и `download_failure_events` в `/data/maksut.db` (без query-параметров, токенов, cookies и сырого stderr):

```sh
docker compose -f compose.yaml exec -T maksut python3 -c 'import sqlite3; db=sqlite3.connect("file:/data/maksut.db?mode=ro", uri=True); print(*db.execute("SELECT f.rowid, f.created_at, f.url_host, f.url_path, e.event_index, e.stage, e.detail, e.exit_code FROM (SELECT rowid, * FROM download_failures ORDER BY rowid DESC LIMIT 30) AS f JOIN download_failure_events AS e ON e.failure_rowid = f.rowid ORDER BY f.rowid DESC, e.event_index"), sep="\n")'
```
