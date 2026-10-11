# Фиксация результатов dev-tracker — 11 октября 2026

Дата заголовка — московская; время измерений ниже — UTC.
Факты по настроенным источникам Solana, BSC, Base и Robinhood Chain
собраны. BSC PnL завершён и отсортирован. Base ATH остаётся частичным:
исторический проход сохранён на курсоре, чтобы оставить квоту Codex
для боевого сервиса. Демон продолжает forward ingestion.

## Что собрано и какой период подтверждён

| Сеть | Launches | Migrations | Подтверждение исторического прохода |
|---|---:|---:|---|
| Solana | 10,087,654 | 162,214 | 365 требуемых закрытых UTC дней create/migrate; 366 markers с одним предыдущим overlap day. |
| BSC | 5,404,828 | 20,999 | Все четыре backwards source cursors = 56,770,511. Approximate block span захватил запас до августа 2025. |
| Base | 11,480,799 | 28 | Все девять event-source cursors достигли нижней границы; минимум 36,627,719, timestamp 2025-10-09T21:19:45Z. |
| Robinhood | 1,280,271 | 10,616 | Все семь ранее запущенных backwards sources дошли до block 0; это отдельный завершённый проход. |

Counts взяты из live БД в 2026-10-10T23:30:00Z **после** снимка;
они включают forward ingestion и запас approximate block-span сканирования.
Это не утверждение, что каждый токен сети или любой launchpad покрыт.
Для Solana требуемое закрытое окно — `[2025-10-10, 2026-10-10)` UTC.
Для EVM фактические нижние блоки и earliest/latest по launchpad сохранены
в [локальном manifest](../../../inputs/results/devtracker-year-finalization-status.json).
Деривация отдельно ограничена 365 днями.

Option-B audit на 22:29:56Z: creator-kind queues и eligible signer queues
пусты; BSC history queue тоже пуста. Старые немигрировавшие BSC запуски
без signer (456,683) сознательно вне eligibility. Robinhood имеет 375
history candidates, но текущий history backend работает только на BSC.
Это сохранённые границы атрибуции, а не потеря записанных launches.
При миграции без matching launch связь также остаётся неизвестной:
предыдущий audit дал BSC 259 и Solana 2,521 unmatched migrations.

## ATH и квота

Base provider-index обработал **154 из 365** закрытых дневных интервалов
Codex `createdAt` filter. Полностью обработаны 2026-05-09…2026-10-09 UTC;
2026-05-08 начат и сохранён для продолжения. ATH в ответах — текущее
накопленное значение на момент запроса, не ATH на конец каждого дня.
Индекс положительных ATH не перечисляет все on-chain контракты.

На 23:29:23Z в Base сохранены **350,746 числовых ATH observations**:
350,205 положительных и 541 числовой ноль. В сумму входят три записи
`codex-index-date-mismatch`: их ATH валиден, но возвращённая дата
не соответствует requested interval. Ещё 642,769 строк `codex-missing`
означают unknown; внутренний scheduling sentinel 0 не превращается
в денежное наблюдение. Отсутствие ATH row также остаётся unknown. Последующий audit (2026-10-11T00:59:55Z) подтвердил, что все Base ATH rows
связаны с сохранёнными launches; index loader пишет только known launch tokens.
Это всё ещё частичное numeric покрытие, не готовность всех топов по раннерам.

Имеющиеся наблюдения сохранены и переиспользуются. Натуральный конец
пагинации не доказывает принадлежность токена к периоду первой пары:
поставщик меняет индекс, возвращённые даты иногда расходятся с фильтром,
и offset pagination не является immutable snapshot. Контракт качества —
[ADR-021, amendments 7–9](../../adr/ADR-021-dev-tracker-facts-and-storage.md).

Последний проход сделал не более 1,439 application attempts, остановился
в 23:25:25Z с exit 4 на лимите своего budget. Остаток общего счётчика —
**1,000**, сохранён для production; счётчик не повышался и не сбрасывался.
Screenshot владельца показывал примерно 7.0K / 10.0K, период Sep30–Oct31;
наш счётчик консервативный и не учитывает другие клиенты аккаунта.
Итоговый статус `ath_partial`, `facts_complete=true`,
`ath_index_complete=false`, `complete=false`.

Демон на source `438e603` читает facts каждые 30 минут, деривирует категории
каждые 60 минут, запрашивает ATH каждые 240 минут с budget 1/chain/pass.
Это максимум 24 scheduled requests/day при четырёх непустых сетях,
744 за 31 день; рестарты и manual jobs расходуют запросы дополнительно.
Прямой ATH путь не делает автоматических HTTP retries. Facts watchdog
сохранил порог 90 минут. Редкий ATH режим ограничивает расход, но не
обещает быстро обновить все eligible токены.

RPC конфигурация этой машины: Helius для Solana; Alchemy primary и Infura
fallback для BSC/Base. Codex используется для market extrema/holders/liquidity,
а не для массового чтения фактов. Исследование бесплатных дополнений —
[free ATH providers](2026-10-11-free-ath-providers.md). Биржевой price high,
pool OHLCV и token-wide lifetime ATH FDV требуют разных coverage policies;
универсальная бесплатная замена для всех четырёх сетей не подтверждена.

## Дополнительный проход кандидатов в раннеры

После первоначального snapshot добавлен opt-in индексный проход с действующими
условиями `top_runners`: ATH FDV $500,000–$10,000,000,000, holders ≥100.
Он перечисляет all-time Codex index и пересекает его с сохранёнными launch facts;
365-дневный период категорий применяется отдельно. Это сокращает расход на
поиск раннеров, но не превращает unknown ATH остальных токенов в ноль.

Base, source `81bc2ea`, 2026-10-11T01:28:55Z: натуральный конец пагинации,
39 из разрешённых 50 запросов, 7,777 provider rows, 3,493 tracked observations.
Из них 121 запуск раньше 2026-05-09. Общий numeric Base count стал 350,898
(+152 относительно сохранённого baseline), поскольку многие observations
обновили прежние строки; 3,493 не означает столько же новых токенов.
Демон продолжал работать, поэтому агрегаты не дают точного per-token diff.
Shared quota после прохода — 959; счётчик не повышался и не сбрасывался.

Robinhood all-time runner index завершён на source `07c7c7e`: 34 запросa,
6,676 provider rows, 1,238 observations для tracked launches; source rows
сейчас уникальны и совпали с process count. BSC all-time и годовой
`tokenCreatedAt >= 2025-10-10` index достигли 10,000 cap каждый и сохранены
как incomplete. Затем три query-only ATH диапазона $500k–$1M,
$1M–$10M, $10M–$10B дошли до натурального конца; общие границы включены
в оба диапазона, idempotent upserts сохраняют единый token key. Для BSC эти
три прохода сделали 81 запрос, прочитали 15,811 provider rows и обработали
1,177 tracked observations; process count может повторять один token. Production
TOML и критерии деривации не менялись. Несовпадений returned token birth
с floor в этих проходах не обнаружено; это provider-filter evidence,
а не доказательство полноты всех токенов сети.

Solana обычный due refresh сделал 27 запросов: 5,333 observations, missing=0
в этом адресном наборе. Старые unknown за пределами due selection этим
не обнуляются и не становятся числовым ATH. Broad годовой runner query упёрся в 10,000 cap и остаётся incomplete.
Три отдельные ATH bands затем завершились: 55 запросов, 10,788 provider rows,
3,378 tracked observations, zero token-birth date gaps. Их union покрывает
тот же ATH range; это не закрывает unindexed/unknown токены.

Из новых BSC кандидатов 14 старых немигрированных запусков через shared
contract были без signer: обычная migrated/recent очередь их не выбирает.
15 Alchemy calls (chain ID 56 + 14 transaction lookups) нашли все 14;
условные updates заполнили только null signer для совпадающих chain/token/
tx_hash/creator с всё ещё shared-contract classification. Использовано
прежнее правило `identity::signer_of`: transaction `from` как launch signer;
это не универсальное доказательство экономического владельца. Raw-safe
отчёт: `.scout-data/diagnostics/bsc-runner-signer-repair-2026-10-11.json`.

Завершён именно filtered provider-index pass. Старый positive-ATH daily pass
остаётся 154/365; полное числовое покрытие каждого токена и полнота lifetime
ATH всех рынков не подтверждены. Typed cursor и quality policy описаны в
[ADR-021, amendment 10](../../adr/ADR-021-dev-tracker-facts-and-storage.md).
Private manifest: `.scout-data/diagnostics/base-runner-index-pass-2026-10-11.json`.

Birdeye ключ принят во всех четырёх сетях, включая Base year-window candles.
Однако сравнение Birdeye daily highs и Codex `athPrice` в одинаковых единицах
USD/token обнаружило большие расхождения. Outlier-toggle BSC/Robinhood и
Solana mcap mode их не устранили. Последующий Solana `outlier=true` открыл пик
в 1.004× Codex `athPrice`, объяснив расхождение этого sample. Отдельный новый
Base probe после сохранённого timeout ответил 200; ранний более высокий peak
всё равно отличается от Codex.
В БД эти альтернативные значения пока не записаны; raw и точные Decimal
comparisons сохранены в [PoC](2026-10-11-birdeye-year-poc.md).

## Robinhood: критерии миграций сохраняются

По решению владельца `top_migr` остаётся ≥80% миграций, минимум три resolved
curve launches, активность за 365 дней. Pending окно — три дня. Никакие
послабления или замена официального списка exploratory-кандидатами не внесены.

Audit 2026-10-11T01:07:12Z: 31,730 dev groups имеют ≥3 resolved curve launches;
ровно 12 проходят 80% и activity gate. Их адреса воспроизводят сохранённый
GMGN список. Проверка всех 480,545 dev groups не нашла нарушений арифметики
migration rate; счётчик migrated не превосходит resolved.
Robinhood backwards cursors дошли до genesis, но самый ранний сохранённый
launch — 2026-06-26: годовое окно включает около 3.5 месяцев наблюдаемой
активности этой сети. Небольшой строгий список сам по себе не доказывает
незавершённый facts pass. Runners остаются отдельной категорией с ATH gates.
Private audit: `.scout-data/diagnostics/robinhood-top-migration-audit-2026-10-11.json`.

## Новые экспорты после runner discovery

[Новая папка](../../../inputs/results/devtracker-exports-2026-10-11T020139Z)
содержит 36 файлов. Все hashes, уникальность и идентичность порядка адресов
между GMGN/Axiom/BasedBot проверены.

| Сеть | top-runners | top-migr | win-streak |
|---|---:|---:|---:|
| Solana | 36 | 416 | 140 |
| BSC | 11 | 74 | 17 |
| Base | 12 | 0 | 0 |
| Robinhood | 12 | 12 | 4 |

Первый [derive JSONL](../../../inputs/results/devtracker-categories-2026-10-11T020139Z.jsonl)
содержит 720 dev verdicts. CLI export пересчитал категории позже: 722 verdicts;
в Solana win-streak добавились два адреса относительно первого derive, прочие
category memberships совпали. Это разные command/time/DB views, не exports
одного immutable snapshot. Старые файлы сохранены; текущие counts выше взяты
из новых экспортов. Деривация и все runner/migration критерии остаются прежними.
Manifest с timestamps, hashes и exact membership deltas:
[runner finalization](../../../inputs/results/devtracker-runner-index-finalization-2026-10-11.json).

Во время первого export PostgreSQL не смог расширить shared-memory segment:
Docker `/dev/shm` 64 MiB был заполнен на 98%. Для новых соединений роли
`scout` в БД `scout` установлен `max_parallel_workers_per_gather=0`; повторный
export завершился, БД не перезапускалась. `scout_test` не затронута.
Docker Compose и Kubernetes PostgreSQL templates теперь выделяют `/dev/shm`
512 MiB; обе конфигурации проверены. В рабочем старом контейнере объём остаётся
64 MiB, serial query workaround действует до переноса/пересоздания. Role setting
не входит в `pg_dump`, поэтому новый deployment должен использовать настроенный
shm mount; на старом контейнере правило применяется отдельно.

Новый custom-format snapshot
`.scout-data/snapshots/devtracker-2026-10-11T020139Z.dump`:
2,837,071,401 bytes, SHA256
`e00a27b166fffef2c55bcfa6bf07267a03f7121022d3bbff7c7116efe7243d52`.
Archive listing и полный isolated PostgreSQL16.15 restore успешны: одна
транзакция, `--exit-on-error`, 10 public tables, schema migrations 1–8.
Проверены source counts, quota 611, complete filtered cursors и все 14 BSC
signer updates; broad 10k-cap cursors сохранили incomplete статус. Container
без сети/ports, memory 4 GiB, shm 512 MiB; после проверки он и его volume
удалены. Отчёт: `.scout-data/diagnostics/runner-snapshot-restore-check-2026-10-11.json`.
Снимок включает факты, новые observations, signer updates и resume cursors.
Quota после всех новых jobs — **611**: расход 389 application reservations
от сохранённых 1,000, без сброса или повышения счётчика. Это собственный
conservative allowance, не актуальный meter аккаунта Codex. При текущем
production ceiling 24 scheduled calls/day около 20 оставшихся дней требуют
до 480 запросов; restarts/manual jobs считаются дополнительно.

По read-only audit 2026-10-11T02:02:04Z numeric ATH / `codex-missing` unknown:
SOL 152,619 / 7,157; BSC 20,603 / 208; Base 350,898 / 642,747;
Robinhood 121,506 / 58,672. Отсутствующая ATH row — дополнительный unknown.
Zero numeric counts не включают scheduling sentinel. Все runner-source rows
связаны с launches; eligible shared signer gaps и повторные creators без
classification на `flap`/`fourmeme`/`pons` равны нулю. Classifier не применяется
к native Solana pump или явно определённым creators остальных launchpads:
отсутствие address-kind там само по себе не считается gap.

## Сохранённые артефакты

[Категории](../../../inputs/results/devtracker-categories-2026-10-10T232525Z.jsonl):
720 dev verdicts, derive start 23:25:25Z / end 23:26:33Z, since_days=365.
[Экспорты](../../../inputs/results/devtracker-exports-2026-10-10T232525Z):
36 файлов — четыре сети, три категории, GMGN/Axiom/BasedBot.
Все file hashes проверены; состав и порядок адресов совпадают между
тремя форматами, наборы адресов совпадают с деривацией.

| Сеть | top-runners | top-migr | win-streak |
|---|---:|---:|---:|
| Solana | 36 | 417 | 140 |
| BSC | 10 | 74 | 17 |
| Base | 10 | 0 | 0 |
| Robinhood | 12 | 12 | 4 |

Это категории создателей из текущего TOML, а не рейтинг кошельков по PnL.
У одного dev могут быть несколько категорий. Пустые Base списки сохранены
как пустые файлы и не заменены произвольными адресами.

Сжатый custom-format snapshot:
`.scout-data/snapshots/devtracker-2026-10-10T232525Z.dump`,
2,836,022,424 bytes (около 2.64 GiB), SHA256
`711860a193ff04e349dd4852086294a24cc24d82705fce377933d555ca3c9fb7`.
`pg_restore --list` и полный изолированный restore успешны.
PostgreSQL16 восстановил архив одной транзакцией с `--exit-on-error`
за 246 секунд (23:30:50Z…23:34:56Z), без опубликованных ports/network.
Проверка в 23:35:35Z подтвердила миграции схемы 1–8, 28,252,044 launches,
193,831 migrations, 1,354,008 ATH rows и 930 cursors. Base ATH source counts,
154 завершённых/1 незавершённый interval и quota 1,000 совпали с manifest.
В снимке на 1,508 launches и 26 migrations меньше последующих live counts:
forward ingestion шёл параллельно. Временный контейнер и его volume удалены;
рабочая БД не изменялась. Полный отчёт проверки —
`.scout-data/diagnostics/final-snapshot-restore-check-2026-10-11.json`.

Dump создаётся из согласованного PostgreSQL snapshot. Derive, export,
dump и последующие counts имеют разные временные/DB views: manifest
хранит отдельные timestamps и hashes, а не обещает, что экспорт получен
из этого dump. Exact registry/finality metadata CLI не выдал; отсутствующие
версии записаны null. Source commit, binary SHA256, Cargo.lock, конфигурация
и helper hash зафиксированы. Facts job использовал `65e8fcf`, финальные
ATH/export и daemon — `438e603`.

BSC таблицы PnL: [human_like 355](../../../inputs/results/bsc_k2_human_pnl30d_by_pnl.csv),
[top-100](../../../inputs/results/bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_by_pnl.csv),
[TPD100 92](../../../inputs/results/bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_tpd100_by_pnl.csv).
SOL: [98 строк](../../../inputs/results/solana_leaderboard_top100_early_k2plus_pnl30d_by_pnl.csv).
Рядом лежат JSON и исходные manifests. Exact sorting, file hashes,
уникальность адресов и TPD100 граница повторно проверены без изменения
замороженных таблиц. Это exploratory known-subset PnL с явными incomplete
statuses; полную прибыль любого кошелька эти таблицы не доказывают.
Разница двух BSC cutoff 20 ч 37 мин 03 с согласована владельцем,
исходные scope/as_of/run_id сохранены в каждой строке.

DDL без данных: [devtracker-db-schema.sql](../../../inputs/results/devtracker-db-schema.sql).
Объяснение архитектуры, выбора технологий и БД для C++ разработчика:
[PROJECT-GUIDE](../../PROJECT-GUIDE.md). История диагностик, изменений
и измерений: [backfill and BSC PnL](2026-10-10-backfill-and-bsc-pnl.md).
Локальные данные и private runtime файлы исключены из git; в git записана
эта сводка, код, fixtures и документация.

## Проверки и следующий шаг

На рабочем source `438e603` успешно выполнены fmt, workspace clippy
all-targets/all-features с `-D warnings`, workspace all-features tests
и release build: **1,171 tests, 99 suites, 0 failed/ignored**.
DB-тесты выполнялись только в `scout_test`; live credentials исключены
из test environment. Пять offline Python-тестов экспортов PnL также прошли.
Логи и exit codes — `.scout-data/checks/2026-10-11/ath-cadence-checks.json`.
Дополнительный Base runner-index source `81bc2ea` также прошёл fmt, workspace
clippy, all-features tests (1,175 tests) и release build; финальный source `f49831d` для всех четырёх сетей, token-birth floor и
проверки Solana mint case прошёл тот же набор (1,178 tests, 99 suites,
0 failed/ignored) и release build. Логи: `.scout-data/checks/2026-10-11/runner-birth-floor-checks.json`. Это не новая версия работающего daemon.

Локальная фиксация **B8.3 завершена**. Следующий task ID: **B8.4** (валидация и интеграция бесплатного ATH complement), **B8.5**
(filtered runner discovery + новые экспорты, в provider scope завершены) и
**B9** (перенос на сервер). Годовой Base ATH не помечается завершённым;
сохранённый provider-index можно продолжить с прежнего курсора при
доступном budget. Текущие transient user services сохраняются при
завершении агента, но постоянный reboot/server deployment ещё не выполнен.

## Изменённые файлы этого продолжения

- `bins/dev-tracker/src/backfill.rs`, `crates/scout-devtracker/src/ath.rs`:
  bounded filtered index, сети, mint case, token-birth floor и quality cursor.
- `crates/scout-devtracker/tests/fixtures/codex_base_runner_filter_2026-10-11.json`,
  `codex_solana_runner_filter_mock.json`: live-safe response shape и synthetic fixture.
- `docs/DEV-TRACKER.md`, `docs/adr/ADR-021-dev-tracker-facts-and-storage.md`:
  interface и provider-only completeness contract.
- `deploy/docker-compose.yml`, `deploy/k8s/postgres.example.yaml`: shm 512 MiB.
- `AGENTS.md`, `docs/TICKETS.md`, этот отчёт и
  `2026-10-11-free-ath-providers.md`: discovered gotchas, evidence и status.
- Research notes: `2026-10-11-birdeye-year-poc.md`,
  `2026-10-11-birdeye-lifecycle-validation.md`,
  `2026-10-11-base-launchpad-ath-research.md`,
  `2026-10-11-public-historical-ath-followup.md`,
  `2026-10-11-listed-coin-ath-crosschecks.md`,
  `2026-10-11-solana-robinhood-ath-sources.md`.

Private helpers, runtime manifests, raw responses, новые exports и snapshot
на диске, исключены из git. Следующая незавершённая задача **B8.4** — политика
и интеграция альтернативного ATH/price источника; затем **B9** — сервер.
Full-token annual numeric ATH не объявлен завершённым.
