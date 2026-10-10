# Продолжение бэкфилла и BSC PnL — 2026-10-10

Состояние сессии фиксируется по проверенным артефактам, курсорам PostgreSQL и логам. BSC PnL завершён; Base-факты и ATH ещё в работе. Время ниже — UTC. Таблицы с адресами и raw JSONL остаются локальными в `inputs/results/`, которые исключены родительским `.gitignore`; в git сохраняются код, тесты и эта сводка.

## Завершённый BSC PnL

Исходная K≥2 discovery-выборка содержит 732 кошелька: 355 human_like, 243 suspect, 134 bot. Расчёт объединяет первый проход 355 кошельков, повтор 57 ошибок и исправленный повтор последнего кошелька. Успешный результат сохраняется при более поздней ошибке. Все входные JSONL проверены по run_meta, последнему run_summary, run_id и полному набору кошельков.

| Артефакт в inputs/results | Строк | Статусы | Положительный PnL |
|---|---:|---|---:|
| `bsc_k2_human_pnl30d_by_pnl.{csv,json}` | 355 | 312 incomplete, 28 ok, 15 no_trade_activity; 0 scan errors | 69 |
| `bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_by_pnl.{csv,json}` | 100 | 86 incomplete, 6 ok, 8 no_trade_activity | 22 |
| `bsc_leaderboard_top100_early_k2plus_with_curve_pnl30d_tpd100_by_pnl.{csv,json}` | 92 | 86 incomplete, 6 ok | 22 |

В общей human_like таблице: 69 положительных, 245 отрицательных и 41 нулевой known-subset PnL; числовой результат есть у всех 355. Полная история отмечена у 43, включая 15 no_trade_activity. В топ-100 полная история у 14, включая 8 no_trade_activity. Эти восемь исключены из TPD100 из-за отсутствия активных дней; порог проверяется точно по `(buys+sells) <= 100*active_days`, `active_days>0`.

Сортировка: realized_net_pnl по убыванию точного Decimal, unknown последним, адрес как tie breaker. Отрицательные и нулевые результаты сохраняются. Исходный discovery rank сохранён отдельно от pnl_rank. Это exploratory таблица по доступному набору сделок, без строгих quality gates `wallet-rank`: incomplete не означает доказанную полную прибыль, неизвестная себестоимость и неподдержанные venues остаются явными в каждой записи.

С согласия владельца объединены два окна 30 дней, с разницей cutoff 20 ч 37 мин 03 с:

- 2026-09-09 18:41:47 — 2026-10-09 18:41:47: 76 строк топ-100;
- 2026-09-10 15:18:50 — 2026-10-10 15:18:50: 24 строки топ-100.

У каждой строки сохранены since/until/as_of/captured_at/run_id/source_id и полная исходная wallet_stats запись. У последнего исправленного повтора cutoff остаётся 15:18:50, но as_of=19:28:20: fetch time и окно не подменены. Источники не записали software/finality/registry version; manifest явно хранит null вместо придуманной версии, а decoder scope, ledger/price policies и quality metadata сохранены в полных source manifests.

Воспроизведение:

```sh
python3 scripts/bsc-pnl-table.py \
  inputs/results/bsc_k2_candidates_with_curve.csv \
  inputs/results/pnl/bsc_k2_human_stats_30d.jsonl \
  inputs/results/pnl/bsc_k2_retry_stats_30d.jsonl \
  inputs/results/pnl/bsc_k2_final_fixed_stats_30d.jsonl \
  inputs/results/bsc_k2_human_pnl30d_by_pnl
python3 scripts/bsc-pnl-top100.py
```

SHA256 raw JSONL:

- первый проход: `fb6789e18640393ba21ea7bd817f783fe6a0aefe61ef97767f3221c0c695b939`;
- повтор 57: `718d9c3ca9934f566b456d4d3e852bddd42553e6010cf83cb8313e5c1b70a3f3`;
- исправленный последний: `37cc1cbd525611a2b5e60c914956ccdbe003f3b14f8c39d29fe20df0c5149122`.

SHA256 итоговых JSON:

- human_like 355: `6aa4d0d59bf1e190588bded3245c9ba64d325533cc2835e4c40c65d4148e41f8`;
- топ-100: `585bbbf4691664487ecf13a8e15216e06f9bc1ec46912472ccdf08d2ce428cc3`;
- TPD100 92: `f4760fb4543e1bca08949038c92510b78381c4b3390c73bb36c94ed8aae9b3a0`.

Пять offline Python-тестов проверяют exact сортировку за пределами 28 значащих цифр, слияние успеха с ошибками/not_scanned, footer validation, top-100 join и границу TPD100.

## Существующие SOL таблицы

`solana_leaderboard_top100_early_k2plus_pnl30d_by_pnl.csv`: 98 рассчитанных строк (84 ok, 14 incomplete). `solana_leaderboard_top100_early_k2plus_pnl30d_tpd100_by_pnl.csv`: 58 строк (56 ok, 2 incomplete). Это ранее рассчитанные результаты, сейчас они не пересчитывались. SHA256 соответственно `61b04c74182c58cc5c38a46b419a800fe495962ac3df5ed645457b9cd97df02e` и `d65276769c56838d8c706274b009757c7df1169d46f2ca152748f4875f56e1c6`. JSON лежат рядом.

## Исправленные причины ошибок

Алерт 20:22/20:25 по Москве означал превышение 90 минут без завершённого шага. BSC identity выполнял истории 500 девов через 66 посредников последовательно; после завершения атрибутировал 4,039 запусков и восстановил heartbeat. Коммит `c024b98` ограниченно распараллеливает независимые истории по concurrency RPC, сохраняет checkpoint каждого завершённого дева и отвергает незавершённую пагинацию на 50-page cap. Это не доказывает полноту старых checkpoints.

Последний BSC scan error вызвала Alchemy listing: одинаковые hash/uniqueId приходили с двумя блоками, а не соседними строками. Канонические transaction/receipt/header на Alchemy и Infura согласовались. Теперь конфликт сверяется ограниченными canonical RPC запросами, timestamp перечитывается, immutable конфликты отклоняются. Golden fixture: `crates/scout-providers/tests/fixtures/bsc_alchemy_conflicting_block_2026-10-10.json`, SHA256 `88d4259375be3f3ddfb3853de9a787acbfd833e312058a701096600c87fca2e2`. Последний кошелёк после исправления: 2,219 транзакций, known-subset realized PnL $274.65836981, incomplete из-за 72 ungated swap logs и 88 IdlOnly trades. Его исходный run_meta ещё содержит прежнюю static coverage note; новое runtime metadata propagation проверяется отдельно, raw артефакт не переписан задним числом.

Выборка Base ATH ускорена индексом migration0008 и NOT EXISTS: подробности, окружение, sampled RSS и ограничения измерения — ADR-021 amendment6. Новый provider-index режим и общий budget — amendment7 и `docs/DEV-TRACKER.md`. [Исследование источников](2026-10-10-base-ath-sources.md) объясняет, почему Alchemy/Infura не являются готовой массовой заменой token-wide ATH FDV.

## Бэкфилл: промежуточная фиксация

Solana: все 365 требуемых закрытых UTC дней launches и migrations отмечены done; в БД 366 markers каждого источника из-за раннего перекрывающегося дня разных запусков. BSC: четыре event source cursors дошли до нижней границы; option-B identity последний batch вернул 0 creators/0signers/0histories. Base: Zora×3, Clanker, Bankr закончены; Noice, Flaunch, Virtuals ещё не закончены.

После отдельной ATH-фазы BSC/Solana (43 запросa BSC, 0 Solana) на 20:02 UTC все найденные migrations с matching launch имеют ATH row: BSC 20,739 из 20,998 migrations (20,531 Codex values, 208 codex-missing); Solana 159,488 из 162,009 (152,337 values, 7,151 missing). Migrations без matching launch не равны потере уже сохранённых фактов: в частности launch может быть вне прочитанного периода; отсутствие связи остаётся coverage gap. Числа меняются при forward ingestion.

`backfill --days` задаёт приблизительный block span. Ранее достигнутый BSC floor 56,770,511 имеет timestamp 2025-08-07T14:12:26Z (запас сверх года), Base floor 36,660,092 — 2025-10-10T15:18:51Z. Derivation ограничивает launch period отдельно; block cursors не подменяют точный календарный scope.

Промежуточный backup: `.scout-data/snapshots/devtracker-2026-10-10-in-progress.dump`, 2,721,716,496 байт, формат PGDMP; SHA256 `2ee57e7a7d1126b2755c6531ac9eeed234abbc3eef12be7ba41ade5ea0b960b6`. `pg_restore --list` PostgreSQL16 успешно читает archive через stdin. Это проверка структуры, не тест полного restore; финальный backup будет сделан после завершения фактов и выбранного ATH-прохода.

Следующий task ID: B8 (Base facts + provider-known ATH + final snapshot), затем B9 (перенос на сервер). Полное числовое ATH для каждого Base контракта не подтверждено: unindexed/untraded/null и границы first-pair scope должны остаться unknown.

## Проверки и файлы

Финальная проверка текущего кода: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features` — успешно; 1,156 tests, 99 suites, 0 failed, 0 ignored. `SCOUT_TEST_DATABASE_URL` указывал исключительно на локальный `scout_test`; live provider credentials исключены из test environment, DB-тесты не пропускались. Пять Python-тестов успешны. Логи и exit codes: `.scout-data/checks/2026-10-10/*-final.log`, `final-checks.json`. CI теперь поднимает собственный PostgreSQL16 `scout_test`, поэтому DB-тесты также выполняются без платных RPC.

Проверка diff и JSON/YAML успешна. Известные runtime credentials не найдены в изменённых/новых файлах (scoped scan, не полный gitleaks-аудит); gitleaks/cargo-audit остаются существующими CI jobs. Runtime dependencies не добавлялись.

Изменённые группы: scout-providers (`evm_alchemy`, `evm_rpc`, `evm_scan`, golden fixture); scout-engine (`evm_wallet_stats`, replay test); scout-devdb (candidate query/index, quota, lookup, unknown projection, DB tests); scout-devtracker (`ath`, live subset fixture), CLI `backfill`; четыре BSC export/test Python scripts; ADR020/021, DEV-TRACKER, TICKETS, AGENTS и CI PostgreSQL service.

Во время продолжения обнаружена остановка прежних фоновых процессов; DB и последние committed cursors сохранились. Новые процессы запущены через transient user systemd services, независимые от process group агента. Причина остановки по имеющимся логам не установлена. Перезапуск переигрывает только незавершённый шаг. В private backfill-config фоновый ATH budget снижен с 20 до 1 request/chain/pass, чтобы сохранить запросы для годового provider-index прохода. Полный server/reboot deployment не заявляется.

Release build `cargo build --release -p dev-tracker -p wallet-stats` успешен. Код зафиксирован: `dac8bcf` (Alchemy reconciliation + runtime coverage notes), `e98e214` (Base ATH index, SQL, quota, unknown projection, CI DB). Новый daemon использует release с общим allowance 4,000, установленным один раз `ON CONFLICT DO NOTHING`; facts-only бэкфилл продолжает текущий шаг без добровольного прерывания.

Автоматическое завершение запущено как `trader-scout-finalize-year.service` (private helper `.scout-data/finalize-year.py`). Оно ждёт успешного footer facts-job и всех девяти Base cursors ≤ 36,669,205, затем запускает до трёх resumable ATH passes с cap 2,000, общим не сбрасываемым allowance, локальные derive/export и проверенный архив. Guard на текущих незавершённых курсорах проверен: HTTP/export/dump не стартовали. Статус/финальный manifest: `inputs/results/devtracker-year-finalization-status.json`; `complete` относится к facts + positive provider-index scope, а не числовому ATH каждого контракта. При quota/error unknown и частичный статус сохраняются; отдельного Telegram send нет.

## Noice: сокращение запросов чеков, 2026-10-11 по Москве

Увеличение receipt concurrency с 8 до 32 не дало убедительного результата:
64 исторических чека в каждой конфигурации, около 6 % разницы при большом
разбросе между двумя группами. Это малый диагностический replay, не измерение
годового потока; рабочая concurrency остаётся 8.

Новый путь получает Create и Lock одним диапазонным запросом по прежнему
initializer. Lock группируется по блоку и transaction index; выбирается прежний
первый same-emitter Lock после Create и прежний крупнейший non-protocol
beneficiary. Если атрибуция не получилась, вызывается прежний receipt decoder.
Определение дева не меняется. Create-only counts, idempotent inserts и cursor
после успешной записи сохранены; оба вида событий входят в прежний cap 200k,
HTTP body ограничен 16 MiB с существующим adaptive splitting.

На 2026-10-10T21:13:24Z диапазон Base 43,428,000…43,429,000 вернул 248 Create
и 248 Lock (464,803 байта). Один запрос логов и один batch из восьми receipt
methods: все восемь Lock совпали по address/topics/data/block/tx index/log index/
tx hash; независимо выбранные beneficiaries совпали 8/8. В этой выборке нет
multi-Create транзакций; этот случай проверяется синтетическим тестом. Это
проверка атрибуции, не экстраполяция производительности или полной сети.

Golden subset из двух реальных транзакций:
`crates/scout-devtracker/tests/fixtures/noice-lock-companions-2026-10-11.json`,
SHA256 `dabea7461cea2be7da24846062ba2449ea24d34d1e16de7181d18e94b3cf6d6a`.
DB-backed mock ingest проверяет отсутствие receipt requests при валидных Lock,
fallback при пропущенном Lock (beneficiary отличается от sender) и отсутствие
движения cursor при cap failure. Offline-тесты также покрывают ordering,
wrong transaction/emitter, malformed first Lock и split oversized response.

Повторные обязательные проверки после изменения: fmt, workspace clippy с
`-D warnings`, workspace all-features tests — успешно (1,161 passed, 99 suites,
0 failed/ignored, DB tests без skips на изолированном `scout_test`). Release
`dev-tracker` собран; результаты и логи —
`.scout-data/checks/2026-10-11/lock-fastpath-checks.json`. Изменены
`scout-devtracker/src/evm_ingest.rs`, его golden fixture, regression test в
`scout-providers/src/evm_rpc.rs`, DEV-TRACKER и эта сводка. Известные runtime
credentials не найдены scoped scan в новых/изменённых source/fixture/docs.
Следующий task ID остаётся B8: закончить Base facts/identities, ATH и snapshot.

Release `01431df` запущен 21:19:44 UTC. Base возобновлён с `--days 366`, чтобы
запас приблизительного block span сохранял прежнюю нижнюю границу после
перезапуска; деривация остаётся 365 дней. Новый floor 36,627,719 проверен RPC:
2025-10-09T21:19:45Z. Счётчик Codex при рестарте остался 3,994; не сбрасывался.
На 21:23:54 UTC первые четыре Noice шага сохранили 143,387 запусков. Это
операционный результат другого диапазона, не контролируемое сравнение скорости
с предыдущим процессом. На 21:24:18 процессный VmHWM = 264,096 KiB: release,
i7-12700K, 16 доступных logical CPU, RAM 24,031,788 KiB, Linux 7.0.0-38,
PostgreSQL16, Alchemy primary с 5,000 CU/s client limiter, параллельно forward
daemon; provider cache неизвестен, процессный timestamp cache изначально пуст.
Это пик до указанного момента, не финальный peak RSS всего бэкфилла.

Соседняя проверка 43,300,000…43,300,999: 751 Create + 751 Lock,
1,349,559 байт, 618 транзакций, из них 111 multi-Create; все 751 дали usable
companion beneficiary. Чеки этой выборки не запрашивались, поэтому byte equality
для её multi-Create случаев не заявляется. Большие 50k-block ответы всё ещё
требуют нескольких splits. Теперь CLI уменьшает шаг Bankr/Noice вдвое при
>5k Create и увеличивает при <2.5k до `--evm-step-blocks`; minimum = 2k или
меньший пользовательский cap. Остальные source steps не меняются. Пять тестов
проверяют density thresholds, bounds и saturation; повторные fmt/clippy/
workspace tests/release успешны (1,166 tests, 99 suites, 0 failed/ignored,
DB без skips). Логи: `.scout-data/checks/2026-10-11/*-adaptive-lock.log`.

Noice завершён на 21:29:44 UTC: cursor 36,627,952; последний запуск процесса
с 21:27:30 добавил 168,165 фактов за 6,100,893 прочитанных блоков (значительная
часть диапазона пустая). Flaunch и Virtuals ещё в работе. Finalizer теперь
клонирует конфигурацию, меняя только output directory, и сохраняет 36 файлов
GMGN/Axiom/BasedBot в отдельный `inputs/results/devtracker-exports-<tag>/`
с SHA256. Exact derive as-of CLI не выводит: manifest сохраняет null с причиной
и start/end bounds; source/frozen config hashes и разные daemon/backfill
software commits записываются отдельно. Clone проверен через `tomllib`: все
поля кроме delivery.out_dir совпали, ожидаемый file count = 36.

Для плотного Flaunch повторён тест настоящего SDK `receipts_by_hashes`, release:
128 разных уже сохранённых hashes в блоках 44,328,856…44,330,388; две группы
по 64, порядок 8→32 и 32→8, всего 256 actual receipt attempts. Все parsed
receipts совпали с входными hashes в исходном порядке; 0 null/errors/retries,
0 limiter waits/halvings. Concurrency 8: 128/2.124 s = 60.3/s; 32:
128/0.788 s = 162.4/s. Runtime peak RSS 11,120 KiB; i7-12700K, Linux7.0,
rustc1.98.1, параллельно основной backfill и daemon. Общий client/budget=256,
2,500 CU/s с burst 2,500, receipt cost20; весь replay 2.912 s / 5,120 CU.
Краткие фазы используют burst, поэтому 162.4/s не объявляется sustained
throughput под этим limiter. Выборка раннего dense subrange, provider cache
мог быть прогрет основным проходом. Диагностика:
`.scout-data/diagnostics/flaunch-sdk-concurrency-2026-10-11.txt`.

На основании этого измерения только Base backfill concurrency увеличена до
32; остальные сети остаются на 8, method-weighted limiter и retry bounds
сохранены. После изменения fmt/clippy/workspace tests/release успешны;
логи `.scout-data/checks/2026-10-11/*-base-concurrency.log`.
