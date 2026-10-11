# ATH: закрытые пропуски и оставшееся покрытие — 2026-10-11

Годовые launch/migration facts для настроенных источников сохранены, но полное
числовое ATH-покрытие не завершено. На 11:07 UTC закрыты **411 пропусков** в
зафиксированной выборке: 334 через Codex, 69 через проверенные окна Birdeye
и 8 через GeckoTerminal с on-chain/FX подтверждением. Последнее продолжение
добавило 66 числовых значений без ручных запросов к Codex.
Это результат адресного заполнения, не новое доказательство полноты всех
токенов сети или канонического lifetime ATH.

## Выборка и фактические записи

Исходные 679 unknown принадлежат сохранённой выборке 720 dev-кандидатов;
сопоставление идёт по прямому creator, с годовым периодом и действующей
ATH-eligibility. Не все signer aliases и новые devs входят в этот audit.

| Сеть | Unknown до прохода | Новые числовые Codex | Проверенные альтернативные FDV | Осталось unknown |
|---|---:|---:|---:|---:|
| Base | 503 | 324 | 53 Birdeye + 5 GeckoTerminal | 121 |
| BSC | 13 | 0 | 10 Birdeye + 3 GeckoTerminal | 0 |
| Robinhood | 53 | 0 | 6 | 47 |
| Solana | 110 | 10 | 0 | 100 |
| Всего | 679 | 334 | 77 | 268 |

Шесть ручных Codex attempts дали 334 числовых наблюдения. Из тех же
сохранённых ответов дополнительно перенесены holders/liquidity для 281
unknown-строки без дополнительных API-вызовов. Значение `codex-missing`
остаётся unknown; его внутренний нулевой sentinel не становится денежным нулём.
Числовые нули и известные значения сохранены.

Из оставшихся 268 unknown у 162 сохранённые holders ниже действующего
порога 100. Ещё 106 могут влиять на раннеры: Base 73, Robinhood 30,
Solana 3. Это приоритизация, не закрытие ATH у остальных токенов. Порог
holders, миграций, ликвидности, числа запусков и plausibility не менялся.
В исходной выборке уже были три Base-токена без строки `ath`; проверка
использует LEFT JOIN и сохраняет их как unknown. Отсутствие записи не
считается числовым закрытием и не уменьшает исходный знаменатель679.

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

Для трёх Solana-токенов разобраны Token-2022 расширения и восстановлены
578 транзакций с точной суммой сжиганий. Mint authority отозвана при создании;
расширения только metadata. Минутные и секундные свечи определили supply
в момент пиков: 1B у двух токенов и 997,060,502.547477 у третьего.
Для меняющегося supply проверены верхние границы FDV остальных свечей;
начальный или текущий supply вместо исторического не подставлялся.
USD price corroboration пока не проходит1%, поэтому три FDV остаются
незаписанными. Дополнительные три GT pool-list GET дали пустые списки;
нового ценового подтверждения нет. Подробнее: [Solana ledger](2026-10-11-solana-ath-supply-ledger.md).

Шесть Robinhood-токенов относятся к Doppler. Теперь проверены точные
runtime/source implementation и immutable clone targets; повторные
Transfer-запросы сохраняют полный диапазон и параметры. Один creation mint
и отсутствие burn подтверждают стабильный1B supply до блока85,674,112,
хотя контракт разрешает последующие burn. У пяти WETH-пар on-chain Swap
ratio и минутный Coinbase ETH/USD согласуются с Birdeye в пределах0.12%.
Verified aeWETH source/runtime и historical proxy slots подтверждают1:1
ETH conversion в каждом наблюдаемом блоке. Эти пять FDV записаны.
Шестой quote оказался NVDA stock token: реальные NVDA/USDG v3 сделки
за одну секунду до и в последующем блоке после целевого Swap, verified
pool runtime/receipts, точная USDG identity из Paxos и traded Kraken
USDG/USD daily range подтверждают Birdeye в пределах0.111%.
Шестой FDV записан отдельно; устаревшие Chainlink marks исключены,
новая поддержка Robinhood v3 в wallet decoder не объявляется. Подробнее:
[Robinhood audit](2026-10-11-robinhood-ath-supply-source-audit.md).

## Продолжение: Base и BSC

Три спорных Base highs дополнительно подтверждены точными v4 Swap событиями,
receipt identity и датированным Kraken USDC/USD. При обоих концах FX диапазона
расхождение ниже1%; записаны три значения с source
`birdeye-window-supply-swap-fx-validated-v1`, `ath_at=NULL`.
Подробнее: [Base cross-check](2026-10-11-base-ath-onchain-crosscheck.md).

BSC добавила13 непустых дневных окон. Теперь всего183 closed-day окна:
105 непустых и78 пустых; intraday окна хранятся отдельно. Для всех13
BSC-токенов exact runtime/verified source плюс mint/burn history подтвердили
1B supply. GeckoTerminal выполнил36 GET без ошибок, по всем возвращённым
первой страницей пулам. Девять цен согласуются по дню и максимуму в пределах1%;
root перепроверил raw SHA, identity, цены и записал9 guarded missing-only FDV.
Оставшиеся четыре расхождения разобраны через receipt-verified PancakeSwap
Swap события и датированный FX: три WBNB-пары согласуются с GeckoTerminal,
четвёртая USD1-пара — с Birdeye. Для USD1 используется закрытый дневной
FX диапазон с реальными сделками; пустая минутная свеча исключена, точная
event-time конвертация остаётся gap. Записаны ещё четыре FDV, все 13
числовых пробелов BSC в этой выборке закрыты. Это не полнота всех токенов BSC.
Все 13 имеют holders<100, поэтому записи не добавляют их в runner list
и не меняют пороги.
Подробнее: [BSC source](2026-10-11-bsc-ath-supply-source-audit.md),
[BSC price](2026-10-11-bsc-ath-price-crosscheck.md),
[BSC on-chain/FX](2026-10-11-bsc-discordant-ath-onchain.md).

Для ещё53 Base Clanker-токенов подтверждены source/runtime с compiler-supplied
immutable offsets и полная сверка100B mint/burn ledger. Ценовой проход сделал
153 GET и затем 14 GET для недостающих возвращённых пулов; все HTTP 200,
без повторов, с интервалом не менее 15 секунд. 38 случаев прошли сверку
дня пика, общего известного максимума и непустого pool scope; записаны
38 guarded FDV. У остальных 15 остались расхождения или пробелы покрытия.
Старые manifests сохранены, дополнительные ответы составляют отдельное
дополнение. Подробнее: [additional Base supply](2026-10-11-base-additional-clanker-supply.md),
[price cross-check](2026-10-11-base-clanker-geckoterminal-crosscheck.md).
Пять из этих 15 разобраны далее по обоим возвращённым USDC v4 пулам:
успешные peak receipts, raw signed deltas, исторические quote decimals6
и датированный Kraken FX согласуются с GT максимумами. Root перепроверил
raw hashes/все Swap ratios и записал5 alternate-GT FDV; остальные10
ещё не закрыты. Очень малые quote notionals сохранены как limitation.
У f3ad одинаковый GT максимум есть Apr25/26, выбран подтверждённый Swap
день Apr25; Birdeye Apr26 не подгонялся. Подробнее:
[Base discordant audit](2026-10-11-base-clanker-discordant-onchain.md).

У шести оставшихся Zora-кандидатов fixed clone target, exact verified
implementation и полный mint/burn ledger подтвердили исторический1B supply.
GT проход сделал10 GET: один случай согласуется по дню/общему известному
максимуму в пределах0.696% и записан. Три цены расходятся, две pool listings
пусты; экстремальный outlier не перенесён в FDV. У трёх спорных пар exact
Initialize и runtime подтвердили content-coin quotes, без доказанного USD FX.
Подробнее: [Zora audit](2026-10-11-base-zora-ath-source-audit.md).

## Код, проверки и операционное состояние

Добавлены bounded Birdeye adapter и CLI `birdeye-prices`, schema 9
`ath_price_windows`, сохранение unknown metadata и guarded
`upsert_missing_ath`. Более поздний пустой Codex refresh теперь не стирает
известный ATH, его дату или источник. `observed_at` после такого refresh
фиксирует последнюю проверку; исходная provenance сохранённого FDV
остаётся в его source/window proof.

Source changes: `9fae1b8`, `41d430f`, `946c688`.
Прошли fmt, workspace clippy all-targets/all-features с `-D warnings`,
workspace tests all-features и release build. Набор после дополнений содержит1199 тестов;
DB-проверки использовали только отдельную `scout_test`. Приватные сводки:
`.scout-data/checks/2026-10-11/{birdeye-fallback,ath-retention}-checks.json`.
Минутный/секундный adapter и golden Swap/FX cases: commit `51dc9bf`;
checks в `.scout-data/checks/2026-10-11/{intraday,robinhood-swap-fx}/checks.json`.
Последний полный набор fmt/clippy/tests:
`.scout-data/checks/2026-10-11/robinhood-stock-swap-fx-final/checks.json`.
Golden cases отдельно проверяют RH ETH/USD, BSC BNB/USDT×USDT/USD
и USD1/USDT×USDT/USD; денежные сравнения выполняются точной арифметикой.
Fixtures и ADR дополнения зафиксированы коммитами `cd513cb`, `17df34c`.

Результаты первого завершённого прохода и последующий экспорт фиксируются в
`inputs/results/birdeye-finalization-2026-10-11.json`. Три отдельных
FDV manifests сохраняют 1 + 3 + 7 guarded writes; private raw evidence
не публикуется в git. Ранние snapshots/exports не переписаны. Общий
Codex request counter не повышался и не сбрасывался; production
cadence остаётся 1 request/chain раз в 240 минут, до 24 scheduled requests
в день. Рестарты дополнительно расходуют attempts.
Продолжение записей: `inputs/results/birdeye-validated-fdv-swap-fx-2026-10-11.json`
(3 Base), `birdeye-validated-fdv-bsc-2026-10-11.json` (9 BSC) и
`birdeye-validated-fdv-robinhood-2026-10-11.json` (5 Robinhood).
Дополнительные записи: `geckoterminal-validated-fdv-bsc-2026-10-11.json`
(3 BSC), `birdeye-validated-fdv-bsc-usd1-2026-10-11.json` (1 BSC) и
`birdeye-validated-fdv-base-additional-{20261011T094659Z,20261011T100346Z,20261011T101452Z}.json`
(16 + 14 + 8 Base). Все 59 значений продолжения включены в новый экспорт ниже;
ранний snapshot 08:01 UTC их не содержит. Его файлы сохранены.
Последние7 записей: `birdeye-validated-fdv-zora-2026-10-11.json` (1 Base),
`geckoterminal-validated-fdv-base-2026-10-11.json` (5 Base) и
`birdeye-validated-fdv-robinhood-stock-2026-10-11.json` (1 Robinhood).
Они появились после checkpoint404 и snapshot10:25 UTC; отдельный
`inputs/results/ath-complementary-finalization-2026-10-11.json` сохраняет
следующий экспорт/снимок с411 закрытыми пробелами.
Дополнительных ручных Codex requests в продолжении не было; демон не перезапускался.

## Ресурсы и CI

Новые списки: `inputs/results/devtracker-exports-2026-10-11T110545Z/`.
Все 36 файлов имеют записанные SHA-256; GMGN/Axiom/BasedBot совпали по
составу и порядку, адреса/уникальность/лимиты проверены. Top-runners:
SOL 36, BSC 11, Base 12, Robinhood 12. Top-migrations: SOL 410, BSC 74,
Base 0, Robinhood 12; win-streak: SOL 142, BSC 16, Base 0, Robinhood 4.
Экспорт обработал 27,473,514 launches, 4,330,647 creators, 717 category members.
Runner membership и порядок сохранены. Между живыми экспортами10:15/11:05
UTC у SOL migrations один адрес вошёл и один вышел, у streak один вошёл и
два вышли. Факты продолжают обновляться, граница365 дней сдвигается;
это не изменение порогов. Все остальные списки совпали по составу/порядку.
Проверки файлов и cohort404/275 сохранены в
`inputs/results/ath-complementary-finalization-2026-10-11.json`.

Checkpoint404 snapshot: `.scout-data/snapshots/devtracker-2026-10-11T102557Z.dump`,
2,839,325,070 bytes, SHA-256
`7acd008dbcd490dbe17acc3f47ca5aa2f7ae83892a05b35c937391b6970a2fa5`.
Проверены SHA, custom archive listing с данными всех обязательных таблиц
и полное декодирование архива через `pg_restore --file=/dev/null`.
Этот snapshot целиком не восстанавливался в отдельную БД; ранее успешно
восстанавливался предыдущий snapshot. Все 59 числовых записей продолжения
включены, private raw proof/config хранятся отдельно. Export и dump используют разные
живые DB views, а не один общий immutable snapshot.

Следующий snapshot с411 закрытыми значениями:
`.scout-data/snapshots/devtracker-2026-10-11T111041Z.dump`, 2,839,464,716 bytes,
SHA-256 `570ad1c1c6b8b367f79161e478100ebeb3adca282a65e0c77a5613c626b437d6`.
Те же archive-list/SHA/full-SQL-decode проверки прошли; полный restore
этого файла не выполнялся. Он включает все66 записей продолжения и
следует после свежего экспорта; views остаются отдельными. Итоговый manifest:
`inputs/results/ath-complementary-finalization-2026-10-11.json`.

Демон активен, исполняет бинарник с ATH-retention fix и проверенным SHA-256;
в этом продолжении он не перезапускался. Более новый локальный release
с intraday CLI собран и проверен отдельно. Локальный Codex counter на
10:34 UTC — 589: ещё четыре штатных attempts, ручных запросов не было.
Это не независимое показание dashboard аккаунта. Счётчик не повышался/не сбрасывался.

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
источники/ценовые расхождения; **B9.1** — release generation через GitHub CI,
**B9.2** — переносимый deployment-каталог и private state bundle с manifest/SHA.
Оба deployment task запланированы, пакет переноса ещё не реализован.

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
