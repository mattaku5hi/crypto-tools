# ATH: закрытые пропуски и оставшееся покрытие — 2026-10-11

Годовые launch/migration facts для настроенных источников сохранены, но полное
числовое ATH-покрытие не завершено. Этот проход закрыл **345 пропусков** в
зафиксированной выборке: 334 через Codex и 11 через проверенные окна Birdeye.
Это результат адресного заполнения, не новое доказательство полноты всех
токенов сети или канонического lifetime ATH.

## Выборка и фактические записи

Исходные 679 unknown принадлежат сохранённой выборке 720 dev-кандидатов;
сопоставление идёт по прямому creator, с годовым периодом и действующей
ATH-eligibility. Не все signer aliases и новые devs входят в этот audit.

| Сеть | Unknown до прохода | Новые числовые Codex | Проверенные Birdeye FDV | Осталось unknown |
|---|---:|---:|---:|---:|
| Base | 503 | 324 | 11 | 168 |
| BSC | 13 | 0 | 0 | 13 |
| Robinhood | 53 | 0 | 0 | 53 |
| Solana | 110 | 10 | 0 | 100 |
| Всего | 679 | 334 | 11 | 334 |

Шесть ручных Codex attempts дали 334 числовых наблюдения. Из тех же
сохранённых ответов дополнительно перенесены holders/liquidity для 281
unknown-строки без дополнительных API-вызовов. Значение `codex-missing`
остаётся unknown; его внутренний нулевой sentinel не становится денежным нулём.
Числовые нули и известные значения сохранены.

Из оставшихся 334 unknown у 175 сохранённые holders ниже действующего
порога 100. Ещё 159 могут влиять на раннеры: Base 120, Robinhood 36,
Solana 3. Это приоритизация, не закрытие ATH у остальных токенов. Порог
holders, миграций, ликвидности, числа запусков и plausibility не менялся.

## Birdeye: цены отдельно от FDV

Адресный проход сохранил все 170 окон кандидатов: **92 непустых ценовых
серии и 78 пустых**. Scope запроса — 366 закрытых UTC-дней
`[2025-10-10, 2026-10-11)`, `1D`, USD, outlier=true. Годовой период
деривации задаётся отдельно. Пустая серия хранится как NULL, не как нулевая
цена. Первая попытка одного адреса упала на корректной цене с масштабом 46;
после исправления exact-decimal parser этот адрес успешно дочитан.

Наличие 366-дневного request window не означает 366 свечей: многие ответы
содержат всего одну–несколько свечей. Более ранний 20-адресный PoC имел
другую правую границу с ещё незакрытой частью дня; его не смешиваем с
этим closed-day scope.

Для 24 Base Clanker-кандидатов проверены точные verified source-файлы и
mint/burn ledger от creation block до подтверждённого Base block 52,456,236.
В каждой записи ровно один initial mint, нет burn, raw supply сверяется с
`totalSupply()`: 100B при decimals=18. Все 34 source-файла остальных 23
контрактов совпадают с reference implementation. Само текущее supply
без этого доказательства не использовалось как историческая константа.

Независимая сверка цен через GeckoTerminal проверяет тот же токен, USD,
UTC-день пика и максимум по известным возвращённым историям пулов.
Допуск сравнения — 1%; это не точность канонического market-wide ATH.
DexPaprika и DexScreener не дали pool IDs для следующей выборки;
идентификаторы получены из Initialize событий закреплённого Base
Uniswap v4 PoolManager. Заключительный проход сделал 53 GET, все HTTP 200,
без повторов и с интервалом не менее 15 секунд.

Из 11 согласующихся ценовых результатов этого последнего прохода два
уже были записаны ранее. Ещё два (`0x92d958…`, `0xfeb1a6…`) имеют пустые
истории части известных пулов: root не перенёс их в ATH. Записаны семь
дополнительных полностью проверенных случаев. Вместе с четырьмя более
ранними случаями это **11 новых Birdeye FDV**. Девять сравниваемых цен
расходятся; у `0x00b0…` сохранены и ценовое расхождение, и пробелы
пулового scope. Подробнее: [price cross-check](2026-10-11-remaining-ath-crosscheck.md).

Записи имеют source `birdeye-window-supply-validated-v1`, а `ath_at=NULL`:
дневная свеча не даёт точного времени сделки. Ценовые окна, источник,
block/hash anchor и доказательства сохранены. Это maximum observed-window
FDV, пригодный для существующей provider-observed модели, с неполным
market scope и возможными thin-market peaks. Проверка одного PoolManager
не доказывает наличие всех рынков во всех протоколах.

## Solana и Robinhood: что не закрыто

Три Solana и шесть Robinhood positive price windows имеют по одной
свече. Для Solana не доказан исторический supply: обычный pump.fun и
Mayhem нельзя свести к предполагаемому 1B.

Шесть Robinhood-токенов относятся к Doppler, не Pons/Flap. На block
85,674,112 все сообщают 1B, decimals=18; token-scoped Transfer ledger
показывает один mint и отсутствие burn. Это consistency check, а не
доказательство отсутствия rebase/нестандартных изменений supply:
verified source/runtime не установлен. Blockscout ответил 403; один
корректный Sourcify v2 lookup не нашёл verified match. Старые v1 и ошибочные
host/path запросы не считаются доказательством отсутствия source у токенов.
FDV этих девяти токенов не записан.

## Код, проверки и операционное состояние

Добавлены bounded Birdeye adapter и CLI `birdeye-prices`, schema 9
`ath_price_windows`, сохранение unknown metadata и guarded
`upsert_missing_ath`. Более поздний пустой Codex refresh теперь не стирает
известный ATH, его дату или источник. `observed_at` после такого refresh
фиксирует последнюю проверку; исходная provenance сохранённого FDV
остаётся в его source/window proof.

Source changes: `9fae1b8`, `41d430f`, `946c688`.
Прошли fmt, workspace clippy all-targets/all-features с `-D warnings`,
workspace tests all-features и release build. Набор содержит 1192 теста;
DB-проверки использовали только отдельную `scout_test`. Приватные сводки:
`.scout-data/checks/2026-10-11/{birdeye-fallback,ath-retention}-checks.json`.

Результаты и последующий экспорт фиксируются в
`inputs/results/birdeye-finalization-2026-10-11.json`. Три отдельных
FDV manifests сохраняют 1 + 3 + 7 guarded writes; private raw evidence
не публикуется в git. Ранние snapshots/exports не переписаны. Общий
Codex request counter не повышался и не сбрасывался; production
cadence остаётся 1 request/chain раз в 240 минут, до 24 scheduled requests
в день. Рестарты дополнительно расходуют attempts.

## Ресурсы и CI

Новые списки: `inputs/results/devtracker-exports-2026-10-11T075729Z/`.
Все 36 файлов имеют записанные SHA-256; GMGN/Axiom/BasedBot совпали по
составу и порядку, адреса/уникальность/лимиты проверены. Top-runners:
SOL 36, BSC 11, Base 12, Robinhood 12. Экспорт обработал 27,475,161 launches,
4,328,917 creators, 722 category members; состав этих runner списков не
изменился по сравнению с предыдущим экспортом, метрики обновились.

Свежий snapshot: `.scout-data/snapshots/devtracker-2026-10-11T080106Z.dump`,
2,838,670,933 bytes, SHA-256
`e8ec6d8d819c7fefcc80f216acbf7ba15df8d3140d2bbf355f7c2c7ead4ab349`.
Custom archive listing проверен и содержит `ath_price_windows`; именно
этот новый snapshot целиком не восстанавливался. Ранее успешно
восстанавливался предыдущий snapshot. Export и dump используют разные
живые DB views, а не один общий immutable snapshot.

После штатного перезапуска демон активен и исполняет новый бинарник с
проверенным SHA-256; fixes сохранены в source commits. Локальный Codex
counter после рестартов — 593, это не независимое показание dashboard
аккаунта. Счётчик не повышался/не сбрасывался.

Для нашего стека подготовлен [compact Compose profile](../../../deploy/docker-compose.compact.yml):
PostgreSQL 1 GiB, daemon 512 MiB, без swap. [Изолированная проверка](2026-10-11-devtracker-compact-memory.md)
завершила годовую деривацию 27.46M launches без OOM; это не совместный
нагрузочный тест трёх сервисов. Низкий work_mem дал около 20 GiB temp I/O,
поэтому RAM-экономия требует дискового запаса и SSD. Профиль не применён
к текущему production PostgreSQL. Kubernetes examples используют те же
CPU/RAM limits и PostgreSQL настройки; PVC увеличен с 10 до 60 GiB для
годовых данных, временных файлов деривации и WAL. Это запас для одной БД,
не размер общего VPS-диска для трёх сервисов.

Для VPS с тремя сервисами и двумя предположительно XGBoost-моделями
ориентир — 8 GiB при инференсе; достаточность зависит от размеров моделей,
батчей и двух других БД. Training на этом VPS не входит в это предположение.
Standalone disk budget проекта 80–100 GB не является суммой для всех трёх
сервисов. Docker stats включает часть file cache: 917 MiB контейнера не
равняется 917 MiB private process memory; измеренный суммарный PSS был
около 255 MiB в конкретном idle snapshot.

[GitHub CI](../../../../.github/workflows/ci.yml) выполняет проверки и тесты,
но отдельной release artifact/image сборки и публикации сейчас нет.
Release binary собирался локально. Следующий task — **B8.4**, оставшиеся
источники/ценовые расхождения и historical supply; **B9** — перенос на VPS.

## Изменённые файлы

- `bins/dev-tracker/{Cargo.toml,src/main.rs,src/birdeye_backfill.rs}`.
- `crates/scout-devdb/{src/lib.rs,tests/db.rs,migrations/0009_ath_price_windows.sql}`.
- `crates/scout-devtracker/{Cargo.toml,src/lib.rs,src/ath.rs,src/birdeye.rs}`.
- `crates/scout-devtracker/tests/fixtures/`: `birdeye_base_daily_ohlcv.json`,
  `birdeye_overview_{base,bsc,robinhood,solana}.json`,
  `birdeye_precision_extreme_scale.json`, `birdeye_validated_fdv_case.json`,
  `codex_ath_null_extrema_metadata.json`.
- `deploy/docker-compose.compact.yml`, `deploy/k8s/{dev-tracker.yaml,postgres.example.yaml}`.
- `docs/{DEV-TRACKER.md,PROJECT-GUIDE.md,TICKETS.md}`,
  `docs/adr/ADR-021-dev-tracker-facts-and-storage.md`.
- `docs/p0/measurements/2026-10-11-{birdeye-fdv-policy,devtracker-compact-memory,remaining-ath-crosscheck,ath-completion-progress}.md`.
