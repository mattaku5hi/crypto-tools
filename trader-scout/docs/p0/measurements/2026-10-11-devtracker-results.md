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
в денежное наблюдение. Отсутствие ATH row также остаётся unknown. Количество строк `ath` —
не процент покрытия launches: provider-index может вернуть и токены
вне зарегистрированных launchpads, а foreign key между таблицами нет.

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
Дальнейшие изменения этого отчёта и путеводителя затрагивают только docs.

Локальная фиксация **B8.3 завершена**. Следующий task ID: **B8.4** (bounded proof-of-concept бесплатного ATH complement) и
**B9** (перенос на сервер). Годовой Base ATH не помечается завершённым;
сохранённый provider-index можно продолжить с прежнего курсора при
доступном budget. Текущие transient user services сохраняются при
завершении агента, но постоянный reboot/server deployment ещё не выполнен.
