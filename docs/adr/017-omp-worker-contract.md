# 2026-10-29 — OMP worker за контрактом

## Context

Оркестратор самоулучшения сам собирал `docker run`, флаги OMP, proxy и
таймауты. Планировщик, ревьюер и реализатор были жёстко привязаны к этой
реализации, поэтому заменить execution-бэкенд без правки control plane было
нельзя. При этом roboomp не является drop-in заменой: он GitHub-ориентирован,
не имеет plan-gate и локального deploy, а его PR-gate завязан на Bun.

## Decision

- `tools/omp_worker.py` — единственная точка запуска OMP-агента.
- Контракт: один JSON-объект на stdin, один JSON-объект на stdout.
  Запрос содержит `job_id`, `role`, `workspace`, `profile_dir`, `prompt`,
  `tools`, `model`, `timeout`, `max_cost`, `thinking` и параметры контейнера.
  Ответ содержит `ok`, `output`, `error`, `timed_out`.
- Worker не читает `LLM_API_KEY`: оркестратор готовит `models.yml` в profile и
  монтирует каталог read-only. Ключ не попадает в argv и job JSON.
- Worker запускает OMP в одноразовом контейнере с `--read-only`, `cap_drop ALL`,
  без сети кроме internal proxy network. Workspace монтируется `rw`, profile
  `ro`.
- Таймаут OMP и удаление контейнера остаются внутри worker. Оркестратор
  дополнительно ставит внешний таймаут и удаляет контейнер, если worker не
  ответил.
- `tools/omp_worker.py` входит в `FORBIDDEN`: self-improve не может менять
  собственный execution-слой.
- Control plane остаётся в Maxwell: очередь, approval-файлы, base snapshot,
  тесты, deploy, health check и rollback. Worker не получает GitHub PAT и не
  пишет в очередь.

## Why not roboomp

RoboMP — готовый GitHub triage/PR-бот, но не execution plane общего назначения.
Его `enhancement`/`proposal` flow не открывает PR, plan approval отсутствует, а
локальный deploy/rollback не входит в scope. Перенос Maxwell в roboomp потребовал
бы форка его task routing, approval и deploy, то есть потери главного
преимущества — готового продукта.

## Consequences

- Замена OMP на RPC-реализацию или другой backend не затрагивает approval,
  deploy и очередь: меняется только worker.
- Контракт можно тестировать без Docker (`build_command`) и с mock subprocess
  (успех, таймаут, cleanup).
- Оркестратор больше не знает флаги OMP и устройство контейнера; он знает
  только job/result.
- Worker не является сетевым сервисом: нет порта, аутентификации и ещё одной
  поверхности атаки.
