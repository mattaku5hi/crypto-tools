# ATH: закрытые пропуски и оставшееся покрытие — 2026-10-11

Годовые launch/migration facts для настроенных источников сохранены, но полное
числовое ATH-покрытие не завершено.

Текущие завершённые изменения и проверки приведены в конце документа;
следующая открытая задача — B8.4. B9.1 ещё требует запуска GitHub workflow,
B9 требует фактического развёртывания на выбранном сервере.

## Текущий аудит — 18:38 UTC

В frozen cohort из 679 ранее unknown строк теперь **430 числовых значений**:
334 Codex, 72 Birdeye, 20 GeckoTerminal и 4 Solana Tracker; осталось 249
unknown, включая 3 строки без `ath`. Все 13
BSC-пропусков закрыты. Это адресное заполнение, не доказательство полноты всех
токенов сети или канонического lifetime ATH.

В 18:38 UTC guarded missing-only записан Base-токен 67d5: `1440697915`
FDV cents по GeckoTerminal, 9 дневных candles, `ath_at=NULL`. Политика
Amendment 22 допускает пустую историю только у известного пула с полным
creation-to-anchor Swap scan и нулём Swap событий; начальный Initialize price
не используется. Четыре настроенных PoolManager pools — ограниченный scope,
не все рынки Base. Независимый same-day USDC/FX check прошёл, но Birdeye
расхождение примерно 14.4× сохранено как unresolved, без усреднения. В БД
сохранены holders 1424 и `max_liquidity_cents=0`; новые provider requests не
делались. Запись: `.scout-data/diagnostics/base-67d5-guarded-promotion-2026-10-11/manifest.json`.

Четыре Solana Tracker значения прошли независимую проверку цены и supply:
8Cc — `$47,952.52`, PEC — `$65,651.14`, 5b2 — `$121,337.50`, 7DGZ —
`$59,125.28`. У всех `ath_at=NULL`, provider recording window и market scope
остаются неизвестными. 5b2 подтверждён ближайшим post-state за 1.990 секунды
до времени provider peak; для 7DGZ подтверждён только уровень цены в той же
секунде, порядок миллисекунд неизвестен. Сохранены holders 4, 7, 4 и 4; они
ниже порога 100, поэтому эти четыре записи не меняют membership текущих
runner lists. Методика двух последних проверок: [Solana price audit](2026-10-11-solana-five-low-burn-price-corroboration.md);
source-price/supply history: [supply audit](2026-10-11-solana-low-burn-peak-supply-history.md).

Checkpoint до записи 67d5: schema-10 snapshot с четырьмя Solana Tracker
записями снят в 17:51:08 UTC и полностью
восстановлен в изолированной PostgreSQL за 490.13 секунды при лимитах 1 GiB,
1 CPU и без сети. Restore подтвердил 28,300,208 launches, 194,654 migrations,
1,357,128 ATH rows, 941 cursors, 100 source observations, quota ledger 104,
и cohort 679/429/250/3 absent. Это checkpoint 429/250, предшествующий 67d5;
его manifests остаются историей, не подтверждением текущего состояния.
Manifest: `.scout-data/diagnostics/schema10-four-solana-snapshot-manifest.json`;
[restore manifest](../../../.scout-data/diagnostics/schema10-four-solana-restore-manifest.json).
36 export files в 18:05 UTC также предшествуют записи 67d5. После commit
`7bd0635` собраны все четыре CLI release binaries; local isolated runtime smoke прошёл без
provider calls, host ports или production writes и не был GitHub/Bookworm
build ([manifest](../../../.scout-data/checks/2026-10-11/b9-2-portable-current/runtime-smoke-manifest.json)).
Предыдущий private bundle проверен и извлечён: 39 files,
2,842,306,560 bytes, SHA-256
`defd186671ad8c9fc5a0fe23271d2fe6ab1b1a957ac2a1937796a33492dfe344`.
Он соответствует checkpoint до 67d5. Новый snapshot на 18:39:37 UTC полностью
восстановлен за 459.63 s при 1 GiB/1 CPU/no network: 28,303,621 launches,
194,715 migrations, 1,357,128 ATH rows, 941 cursors, 229 price windows,
100 exact observations, quota 7+97 и frozen 430/249 проверены. Все пять
guarded values и новый GT window совпали; test container/volume удалены.
Из него собран 39-file private bundle (2,842,726,400 bytes, SHA-256
`33919254447f4edceea78fb70cf40f0684aa31b87b1084775b343f020e2e7d87`);
verify/extract/independent hashes прошли за 19.82 s. Current release binaries
source de3fd42 прошли Bookworm help и isolated runtime smoke. Config
использует относительный путь `exports`, секреты передаются отдельно.
Подробности и пути:
[portable validation](2026-10-11-portable-deployment-validation.md).

| Сеть | Числовые значения из исходных unknown | Осталось unknown |
|---|---:|---:|
| Base | 397 / 503 | 106 |
| BSC | 13 / 13 | 0 |
| Robinhood | 6 / 53 | 47 |
| Solana | 14 / 110 | 96 |
| Всего | 430 / 679 | 249 |

Актуальный экспорт на 18:42 UTC:
`inputs/results/devtracker-exports-2026-10-11T184232Z/`.
Топ по миграциям содержит SOL 413, BSC 75, Robinhood 12, Base 0;
топ по раннерам — SOL 36, BSC 11, Robinhood 12, Base 12.
BSC wallet PnL отдельно проверен: 355 rows в
`inputs/results/bsc_k2_human_pnl30d_by_pnl.{csv,json}`, сортировка exact-decimal
realized USD DESC. Это exploratory known-subset PnL с per-wallet windows
и coverage gaps, без применения `wallet-rank` quality gates.

После записи 67d5 остаётся 91 potentially relevant gap: 5 Base с известными
holders≥100 и 86 с holders=NULL. Ещё 158 имеют known holders<100.
[Датированный triage](2026-10-11-remaining-ath-triage.md) сохраняет 250-row
checkpoint до этой записи. RH30 empty-window tokens имеют initialized
Doppler pools без наблюдённых Swap в audited scope; это отсутствие цены,
не числовой ноль и не доказательство отсутствия других рынков.
[RH protocol audit](2026-10-11-rh-doppler-empty-window-protocol-audit.md),
[Base price-kind recheck](2026-10-11-base-prior10-boundary-price-recheck.md).

## Исторический checkpoint: 17:12 UTC

На этом этапе было закрыто **427 пропусков** в зафиксированной выборке:
334 через Codex, 72 через проверенные окна Birdeye, 19 через GeckoTerminal
с on-chain/FX подтверждением и 2 через Solana Tracker после независимой
сверки цены и доказательства supply. На этапе до 12:03 добавлены 80 значений;
эта таблица сохраняет состояние до следующих двух Solana Tracker записей.

## Выборка и фактические записи

Исходные 679 unknown принадлежат сохранённой выборке 720 dev-кандидатов;
сопоставление идёт по прямому creator, с годовым периодом и действующей
ATH-eligibility. Не все signer aliases и новые devs входят в этот audit.

| Сеть | Unknown до прохода | Новые числовые Codex | Проверенные альтернативные FDV | Осталось unknown |
|---|---:|---:|---:|---:|
| Base | 503 | 324 | 56 Birdeye + 17 GeckoTerminal | 106 |
| BSC | 13 | 0 | 10 Birdeye + 3 GeckoTerminal | 0 |
| Robinhood | 53 | 0 | 6 | 47 |
| Solana | 110 | 10 | 4 Solana Tracker | 96 |
| Всего | 679 | 334 | 96 | 249 |

Шесть ручных Codex attempts дали 334 числовых наблюдения. Из тех же
сохранённых ответов дополнительно перенесены holders/liquidity для 281
unknown-строки без дополнительных API-вызовов. Значение `codex-missing`
остаётся unknown; его внутренний нулевой sentinel не становится денежным нулём.
Числовые нули и известные значения сохранены.

На этапе 12:03 из 254 unknown у 162 сохранённые holders были ниже действующего
порога 100. Ещё 92 требуют проверки для раннеров: Base 59, Robinhood 30,
Solana 3. Это приоритизация, не закрытие ATH у остальных токенов. Порог
holders, миграций, ликвидности, числа запусков и plausibility не менялся.
В числе92 есть строки с holders=NULL: они не считаются прошедшими порог100.
В исходной выборке уже были три Base-токена без строки `ath`; проверка
использует LEFT JOIN и сохраняет их как unknown. Отсутствие записи не
считается числовым закрытием и не уменьшает исходный знаменатель679.
Два последующих SOL-закрытия на этом checkpoint имеют 4 и 7 holders; тогда среди
252 unknown оставались 160 с известными holders ниже100 и прежние92,
требовавшие проверки. Четыре текущих Solana записи имеют holders 4, 7, 4 и 4,
все ниже порога; runner membership от них не меняется.

## Последующее продолжение: Solana Tracker, scope и перенос

Все 100 оставшихся SOL получили точные source-price observations: 97 новых
GET и 3 ранее сохранённых ответа. Учтённое использование приложения —
104 запроса из выделенных250 в октябре; allowance владельца2500/month не
приравнивается к нашему счётчику. У всех100 проверены birth receipts,
отзыв mint authority и более поздний finalized supply snapshot; 49 минтов
выпущены с1B и51 с2B, поэтому blanket1B denominator исключён.

У 8Cc и PEC границы возможного FDV при максимальной цене поставщика
округляются к одной сумме cents. Независимая сверка со стартовыми/ближайшими
после миграции PumpSwap состояниями и датированным SOL/USD подтверждает
обе цены в пределах1% при обоих концах minute FX. Записаны `$47 952.52`
и `$65 651.14` под source
`solana-tracker-window-supply-bounds-pool-fx-validated-v1`, `ath_at=NULL`;
unknown provider window и market scope сохранены. Guards затронули только
два `codex-missing` sentinel rows, holders/liquidity не изменены.
Для PEC порядок первой продажи относительно vendor milliseconds неизвестен;
это сверка близкой цены, не точное восстановление pool state в эти millis.
[8Cc](2026-10-11-solana-8cc-migration-pool-spot-corroboration.md),
[PEC](2026-10-11-solana-pec-migration-spot-corroboration.md).

Дополнительная проверка актуальных топов выявила восемь eligible launches
без ATH вне frozen679: шесть Base/Zora и два уже мигрированных SOL/Pump.
Ещё142 absent-ATH rows сейчас не выбираются по migration/pending rules;
их не прибавляем к историческому знаменателю. Все150 имеют holders=NULL.
Это отдельный live scope, а frozen audit не доказывает полноту новых devs
или всех signer aliases. [Аудит scope](2026-10-11-current-top-runner-ath-gap-audit.md).

Base empty-window аудит обнаружил36 токенов со Swap в настроенном v4 scope
и42 без наблюдавшихся там Swap. Для14 восстановлены indirect USD routes
и event-time supply. Тринадцать Oct9 marks после почти полного выкупа
990M market allocation экстремальны; на их основе ATH/раннеры не добавлялись.
Для e465 получена проверяемая оценка `$112.60` на observed trade boundary,
но independent candle history пуста и полный ATH не доказан.
[14 boundary cases](2026-10-11-base-zora-14-boundary-audit.md).

Полный schema10 snapshot на15:25 UTC восстановлен в отдельной БД: все100
source observations, quota ledger104,941 курсор и прежние425/254 проверены.
Он предшествует двум последующим FDV-записям. Из него собран и извлечён
39-file private deployment bundle; daemon отдельно прошёл startup/export/
clean-stop smoke из `/tmp` с internal network и без provider calls.
[Перенос](2026-10-11-portable-deployment-validation.md).
Workspace fmt, Clippy и1210 тестов прошли перед двумя guarded writes;
код/evidence закреплены в54472c5, перенос — в974a015. B8.4 остаётся
незавершённой задачей; эти результаты не заявляют полного annual/lifetime ATH.

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
Дополнительная Coinbase SOL/USD минутная свеча пересекается с Kraken,
но расхождение остаётся выше1% при обоих концах диапазона; смена FX venue
не закрыла пробел. Подробнее: [FX venue check](2026-10-11-solana-fx-venue-crosscheck.md).
Три public Solana Tracker token-page GET также не дали числового ATH
в HTML. На момент этого keyless прохода free-plan entitlement ещё
не был проверен; последующий authenticated проход описан ниже:
[public pages](2026-10-11-solana-public-token-ath-pages.md).
После добавления владельцем Data API key выполнены3 ATH GET, все HTTP200.
Фактический allowance этого аккаунта по данным владельца —2500 requests/month;
рекламные10000 из docs не используются для бюджета. Цены и millisecond
timestamps сохранены отдельно. `highest_market_cap` у этих ответов совпадает
с price×позднейшим supply, а не подтверждённым market cap в момент пика;
автоматического переноса в ATH нет. Birdeye1D highs совпадают с его1s highs
и также расходятся с ST. Пулы двух токенов котируются в87ea, не в SOL/USD.
Подробнее: [keyed probe](2026-10-11-solana-tracker-ath-keyed-probe.md),
[cached peak transactions](2026-10-11-solana-tracker-pump-swap-peak-validation.md).
Ещё1 ST TokenInfo GET и8 Helius RPC нашли exact87ea/wSOL pool и nearby
quote-vault observations: всего4 ST requests за эту проверку, без повторов.
`MultiHopSwap`/`SellV2` требуют отдельной version/semantics проверки;
`BoostBuyAndBurn` целевых токенов не считается приобретением пользователя.
Во вспомогательном private spot расчёте были пропущены virtual quote reserves;
оригиналы сохранены, отчёты исправлены, неверные значения в БД не попали.
Production decoder уже сохраняет это поле, SDK bug не установлен.
Для первого mint v3 восстановил post-virtual state по закреплённой формуле
комиссий:12 событий согласуются с vault deltas,18 состояний рассчитаны.
Ближайший coherent spot остаётся ниже ST на2.39–2.47%;1% gate не пройден.
Историческая deployment revision и привязка provider clock остаются scope gaps.
Подробнее: [v2 reconciliation](2026-10-11-solana-virtual-reserve-price-reconciliation.md),
[v3 state reconstruction](2026-10-11-solana-virtual-reserve-price-reconciliation-v3.md),
[quote87 bridge](2026-10-11-solana-quote87-pool-bridge.md).
Последующая version-matched сверка JC→87ea→wSOL successful atomic route
подтвердила цену ST в пределах0.375–0.439% при обоих концах minute FX.
Поставщик сообщает price peak, а его market-cap поле в этом ответе совпадает
с price×позднейшим supply. Это не доказывает максимум исторического FDV:
более низкая цена при supply до сжигания могла дать больший FDV.
81 post-Boost spot также численно совпал в пределах1%, но quote bridge
взят из отдельной транзакции через6.967s и не исключает direct vault transfers.
Новых FDV записей нет. Подробнее: [JC bracket](2026-10-11-solana-quote87-jc-bracket.md),
[81 spot](2026-10-11-solana-81-nearby-spot-comparison.md),
[price peak versus FDV](2026-10-11-solana-tracker-price-peak-vs-fdv.md).
Три коротких chart GET проверили ряд JC и единицы/время: итого7 ST requests
за диагностику (3 ATH,1 TokenInfo,3 chart), без автоматических повторов.
Контроль без `marketCap`/`timezone` вернул USD цены и43 метки внутри
запрошенного347-секундного span; максимальная цена точно совпала с ATH.
На43 возвращённых строках FDV максимум равен$227310.994248, но ±1s
проверка supply даёт верхнюю границу$227590.653586. Промежутки до75s,
неизвестная семантика пропусков и порядок внутри секунды не позволяют
объявить максимум всего периода. Ответ с `timezone=UTC` имел epoch shift−7200s
и исключён; исходный chart с capitalisation-scale values сохранён отдельно.
Все3 FDV остаются unknown. [Chart/supply bounds](2026-10-11-solana-jc-chart-supply-bounds.md).

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
на этом checkpoint ещё не были закрыты. Очень малые quote notionals сохранены как limitation.
У f3ad одинаковый GT максимум есть Apr25/26, выбран подтверждённый Swap
день Apr25; Birdeye Apr26 не подгонялся. Подробнее:
[Base discordant audit](2026-10-11-base-clanker-discordant-onchain.md).
Следующие2 cross-day случая и7 оставшихся расхождений также прошли проверку
по собственному дню GT-максимума, всем известным USDC Swap ratios,
успешным peak receipts и обоим концам дневного Kraken FX; записаны9 GT FDV.
У92d дополнительный ZORA-quote pool остаётся явно unconverted scope gap,
а его возвращённая GT USD история включена в сравнение известных pool highs.
У a0ad дочитана третья pool history и записан ещё1 Birdeye FDV. Теперь
у всей дополнительной группы53 Clanker-токенов есть числовые значения.
Исходные raw manifests сохранены; нормализованное дополнение исправляет
ошибочное derived поле Kraken low (правильный OHLC индекс3, high —2).
Подробнее: [cross-day](2026-10-11-base-clanker-cross-day-onchain.md),
[last gaps](2026-10-11-base-clanker-last-gaps-onchain.md),
[a0ad scope](2026-10-11-base-a0ad-pool-scope.md).

Из десяти оставшихся Clanker-токенов более ранней группы24 ещё4 прошли
полную сверку:2 Birdeye FDV и2 GT FDV записаны guarded missing-only.
Проверены163 архивных raw файла; exact supply100B/18, все известные
USDC pools, receipts/header/decimals и same-day FX сохранены в proof v2.
Предварительный v1 proof имел неправильные observation/window metadata;
он не использовался для записи, исправленный v2 хранится отдельно.
На этом checkpoint остальные6 оставались unknown. У67d все6 native-pool
события после dated FX ниже USDC пика, но у одного известного USDC пула пустая
GT история; nonempty-history gate для alternate GT ещё сохранялся. У fec3 GT день пика
не совпадает с on-chain максимумом; соседний день не устранил расхождение.
Подробнее: [prior10](2026-10-11-base-clanker-prior10-onchain.md),
[native/date follow-up](2026-10-11-base-clanker-prior10-followup.md),
[all-six native bound](2026-10-11-base-clanker-67d5-native-fx-bound.md).
В 18:38 UTC 67d5 был закрыт отдельным guarded missing-only GeckoTerminal
window write по Amendment 22; пять остальных кандидатов этой группы не
переносятся без своей независимой проверки.
Offline сверка669 cached logs (660 nonzero opposite-delta Swaps) через
post-swap `sqrtPriceX96` отделила marginal spot от execution ratio. У3607
совпадение с Birdeye наMay3 не является общим максимумом: более высокий
известный spot полученMay4, а GT относит высокий уровень кMay5. Остальные
пять также не закрыли date/price/scope gaps; значения в БД не менялись.
Подробнее: [prior-six spot audit](2026-10-11-base-prior6-v4-spot-audit.md).

У шести оставшихся Zora-кандидатов fixed clone target, exact verified
implementation и полный mint/burn ledger подтвердили исторический1B supply.
GT проход сделал10 GET: один случай согласуется по дню/общему известному
максимуму в пределах0.696% и записан. Три цены расходятся, две pool listings
пусты; экстремальный outlier не перенесён в FDV. У трёх спорных пар exact
Initialize и runtime подтвердили content-coin quotes, без доказанного USD FX.
Подробнее: [Zora audit](2026-10-11-base-zora-ath-source-audit.md).
Дополнительный keyless проход нашёл USD histories точных content-coin quotes,
но произведение дневных highs не является ценой одной сделки. Для DVA
две реальные v4 сделки одной транзакции дают точную MATRY conversion;
Coinbase minute ETH/USD показывает расхождение2.55–2.57% с Birdeye,
поэтому FDV не записан. У двух пустых token listings DexScreener также
не нашёл пары. Подробнее: [five-case follow-up](2026-10-11-base-zora-five-price-followup.md),
[DVA/MATRY pilot](2026-10-11-base-zora-dva-matry-pilot.md).
Для трёх ранних Robinhood-кандидатов с неизвестными holders GT списки
пусты, а DexPaprika вернул address-matched metadata с NULL price stats.
Это scoped index gap, не доказательство отсутствия сделок или нулевая цена:
[Robinhood pilot](2026-10-11-robinhood-empty-price-pilot.md).

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
После checkpoint411 записаны ещё14 значений: a0ad Birdeye1,
GT cross-day2, GT last-gaps7 и prior4 (Birdeye2 + GT2).
Recording artifacts: `birdeye-validated-fdv-base-a0ad-2026-10-11.json`,
`geckoterminal-validated-fdv-base-{cross-day,last-gaps}-2026-10-11.json`,
`ath-validated-fdv-base-prior4-2026-10-11.json` в `inputs/results/`.
Свежий checkpoint425 фиксируется отдельно в
`inputs/results/ath-validated-checkpoint-2026-10-11.json`; предыдущие
exports/snapshots остаются неизменными.
Дополнительных ручных Codex requests в продолжении не было; демон не перезапускался.

## Ресурсы и CI

Новые списки: `inputs/results/devtracker-exports-2026-10-11T120327Z/`.
Все 36 файлов имеют записанные SHA-256; GMGN/Axiom/BasedBot совпали по
составу и порядку, адреса/уникальность/лимиты проверены. Top-runners:
SOL 36, BSC 11, Base 12, Robinhood 12. Top-migrations: SOL 412, BSC 74,
Base 0, Robinhood 12; win-streak: SOL 142, BSC 16, Base 0, Robinhood 4.
Экспорт обработал 27,473,164 launches, 4,331,283 creators, 719 category members.
Runner membership и порядок сохранены. Между живыми экспортами11:05/12:03
UTC у SOL migrations два адреса вошли, у streak один вошёл и один вышел.
Факты продолжают обновляться, граница365 дней сдвигается; пороги сохраняются.
Все остальные списки совпали по составу/порядку. Новый ATH изменил метрики
внутри runner names, но не wallet membership. Проверки36 файлов и cohort425/254
сохранены в `inputs/results/ath-validated-checkpoint-2026-10-11.json`.
Предыдущие checkpoint404 и411 manifests/exports сохранены отдельно.

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

Checkpoint425 snapshot: `.scout-data/snapshots/devtracker-2026-10-11T120817Z.dump`,
2,839,741,603 bytes, SHA-256
`dfe620f46d5884cc89796d6f0b1f75f9e4b6f867a2300384a44ad13d41afba08`.
SHA и archive-list совпали. Полное восстановление в отдельный PostgreSQL16.15
прошло с exit0 за8m17.64s: схема1–9, все обязательные таблицы,228 price windows,
baseline679/425/254/92/3absent и четыре последние записи/window proofs проверены.
Сохранён941 cursor из восстановленного snapshot, без сравнения с изменяемой
живой БД. Тестовый контейнер без сети/портов имел1GiB/no-swap и1CPU;
cgroup peak≈1GiB включает filesystem cache, OOM events0. Это не PostgreSQL RSS.
Тестовый контейнер и его volume удалены. Снимок включает80 числовых записей
продолжения; export и dump остаются разными DB views. Полные доказательства:
[schema9 restore](2026-10-11-snapshot-schema9-restore-b84.md),
`inputs/results/ath-validated-checkpoint-2026-10-11.json`.

Демон активен, исполняет бинарник с ATH-retention fix и проверенным SHA-256;
в этом продолжении он не перезапускался. Более новый локальный release
с intraday CLI собран и проверен отдельно. Локальный Codex counter на
12:16 UTC —585: за продолжение прошли восемь штатных attempts, ручных запросов не было.
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

[GitHub CI](../../../../.github/workflows/ci.yml) выполняет проверки и тесты;
release workflow создан локально (commit `8787c5b`), но GitHub workflow ещё не
запускался. **B8.4** остаётся в работе из-за непроверенных ценовых
расхождений. B9.1 реализован локально; B9.2 ожидает упаковку полного текущего
private state bundle и передачу на сервер.

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

## Зафиксированные файлы и проверки продолжения

Коммиты: `be442ee`, `8787c5b`, `b21db97`, `974a015`, `54472c5`, `7bd0635`, `cbb5997`, `6c7170e`, `de3fd42`. Текущий
код прошёл workspace fmt, Clippy all-targets/all-features с `-D warnings`,
1 213 offline tests с отдельной `scout_test`, и release build `--locked`
для всех четырёх CLI. Python/shell/Compose проверки переноса, Bookworm
help/runtime smoke, полное restore и bundle checks описаны отдельно выше.
Known-credential scan прошёл для публичных файлов и выбранного приватного
пакета; это не заявляет secret scan декомпрессированного PostgreSQL dump.

Изменённые файлы этой цепочки (workflow расположен в родительском git root):

- `.github/workflows/trader-scout-release.yml`
- `AGENTS.md`
- `Cargo.lock`
- `README.md`
- `bins/dev-tracker/Cargo.toml`
- `bins/dev-tracker/src/main.rs`
- `bins/dev-tracker/src/solana_tracker_backfill.rs`
- `crates/scout-devdb/migrations/0010_provider_ath_observations.sql`
- `crates/scout-devdb/src/lib.rs`
- `crates/scout-devdb/tests/db.rs`
- `crates/scout-devtracker/Cargo.toml`
- `crates/scout-devtracker/src/birdeye.rs`
- `crates/scout-devtracker/src/lib.rs`
- `crates/scout-devtracker/src/solana_tracker_ath.rs`
- `crates/scout-devtracker/tests/fixtures/geckoterminal_base_zero_swap_pool_fdv_case.json`
- `crates/scout-devtracker/tests/fixtures/solana_tracker_ath_48.json`
- `crates/scout-devtracker/tests/fixtures/solana_tracker_ath_81.json`
- `crates/scout-devtracker/tests/fixtures/solana_tracker_ath_jc.json`
- `crates/scout-devtracker/tests/fixtures/solana_tracker_peak_supply_fdv_bounds.json`
- `crates/scout-devtracker/tests/fixtures/solana_tracker_scoped_spot_supply_cases.json`
- `crates/scout-devtracker/tests/fixtures/solana_tracker_supply_fdv_cent_bounds.json`
- `crates/scout-devtracker/tests/fixtures/solana_tracker_two_price_gate_cases.json`
- `deploy/.env.example`
- `deploy/package-release.sh`
- `deploy/portable/.dockerignore`
- `deploy/portable/README.md`
- `deploy/portable/compose.compact.yml`
- `deploy/portable/compose.yml`
- `deploy/portable/prepare-runtime.sh`
- `deploy/portable/runtime.Dockerfile`
- `deploy/state_bundle.py`
- `deploy/test_prepare_runtime.py`
- `deploy/test_state_bundle.py`
- `docs/DEV-TRACKER.md`
- `docs/PROJECT-GUIDE.md`
- `docs/TICKETS.md`
- `docs/adr/ADR-021-dev-tracker-facts-and-storage.md`
- `docs/p0/measurements/2026-10-11-ath-completion-progress.md`
- `docs/p0/measurements/2026-10-11-ath-free-provider-recheck.md`
- `docs/p0/measurements/2026-10-11-base-e465-gt-crosscheck.md`
- `docs/p0/measurements/2026-10-11-base-e465-onchain-fdv-certificate.md`
- `docs/p0/measurements/2026-10-11-base-e465-supply-source-audit.md`
- `docs/p0/measurements/2026-10-11-base-prior10-boundary-price-recheck.md`
- `docs/p0/measurements/2026-10-11-base-robinhood-ath-free-source-followup.md`
- `docs/p0/measurements/2026-10-11-base-six-triage-67d5-gt-source-audit.md`
- `docs/p0/measurements/2026-10-11-base-zora-14-boundary-audit.md`
- `docs/p0/measurements/2026-10-11-current-top-runner-ath-gap-audit.md`
- `docs/p0/measurements/2026-10-11-empty-cohort-v4-quote-route-audit.md`
- `docs/p0/measurements/2026-10-11-empty-cohort-v4-scope-audit.md`
- `docs/p0/measurements/2026-10-11-empty-window-pool-discovery.md`
- `docs/p0/measurements/2026-10-11-portable-deployment-validation.md`
- `docs/p0/measurements/2026-10-11-pump-coins-v2-ath-fields.md`
- `docs/p0/measurements/2026-10-11-remaining-ath-triage.md`
- `docs/p0/measurements/2026-10-11-rh-doppler-empty-window-protocol-audit.md`
- `docs/p0/measurements/2026-10-11-solana-8cc-migration-pool-spot-corroboration.md`
- `docs/p0/measurements/2026-10-11-solana-8cc-quote-reserve-inference.md`
- `docs/p0/measurements/2026-10-11-solana-cent-bound-price-check.md`
- `docs/p0/measurements/2026-10-11-solana-five-low-burn-price-corroboration.md`
- `docs/p0/measurements/2026-10-11-solana-low-burn-peak-supply-history.md`
- `docs/p0/measurements/2026-10-11-solana-pec-migration-spot-corroboration.md`
- `docs/p0/measurements/2026-10-11-solana-remaining-birth-receipts.md`
- `docs/p0/measurements/2026-10-11-solana-remaining-mint-audit.md`
- `docs/p0/measurements/2026-10-11-solana-runner-potential-supply-window-audit.md`
- `docs/p0/measurements/2026-10-11-solana-tracker-100-source-observations.md`
