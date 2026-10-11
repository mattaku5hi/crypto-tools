# Бесплатные источники ATH для четырех сетей — 2026-10-11

## Краткий вывод

Поиск и интеграция продолжаются. Этот документ фиксирует проверенные источники,
а не завершение годового ATH-филлинга или готовность топов по раннерам.
События запусков/миграций и наличие ATH — отдельные этапы.

Checkpoint12:03 UTC: из679 исходных числовых пробелов закрыты425,
включая72 validated Birdeye и19 validated GeckoTerminal FDV; остаются254.
Для альтернативных записей проверяются исторический supply, priced-token
identity, день максимума и независимая цена/Swap+FX; дневное окно запроса
не является доказательством полноты свечей. Новые quote paths исследованы
в [Zora five-case follow-up](2026-10-11-base-zora-five-price-followup.md):
точные content-coin quotes имеют USD histories, но daily-high products
не являются observed target prices. Реальная DVA/MATRY/WETH цепочка
одной транзакции с minute ETH/USD [расходится с Birdeye на2.55–2.57%](2026-10-11-base-zora-dva-matry-pilot.md)
и не прошла действующий1% gate. Coinbase SOL/USD также [не устранил
расхождение](2026-10-11-solana-fx-venue-crosscheck.md). У трёх ранних
Robinhood-кандидатов [нет price/pool пути в scoped public lookups](2026-10-11-robinhood-empty-price-pilot.md);
NULL и пустые listings сохраняются как gaps.
Public Solana Tracker pages трёх спорных mint не дали ATH в сохранённом HTML;
после добавления владельцем Data API key официальный ATH endpoint ответил
HTTP200 для всех трёх mint. Фактический лимит этого аккаунта —2500 requests/month
по данным владельца, а не рекламные10000. Использованы7 запросов:3 ATH,1
TokenInfo для поиска quote-пула и3 коротких chart/unit/time controls;
автоматических повторов не было. Новых аккаунтов не создавали.
Числовые ATH пока не перенесены: цены требуют независимого подтверждения,
а `highest_market_cap` совпадает с peak price×позднейшим supply у этих ответов.
Два target-пула котируются в87ea, не SOL/USD; [Helius bridge](2026-10-11-solana-quote87-pool-bridge.md)
дал nearby execution quotes, но новые instruction variants и временное
расхождение остаются gaps. [Keyed probe](2026-10-11-solana-tracker-ath-keyed-probe.md),
[public-page audit](2026-10-11-solana-public-token-ath-pages.md).
Поздняя сверка [подтверждает цену JC реальной atomic route](2026-10-11-solana-quote87-jc-bracket.md)
в пределах0.44%. [81 nearby spot](2026-10-11-solana-81-nearby-spot-comparison.md)
также численно согласуется, с явным временным gap между пулами. Для меняющегося
supply нужны совместные наблюдения price/supply: цена на историческом максимуме
не определяет максимум FDV. Это ограничение остаётся после ценовой сверки.
[JC chart controls](2026-10-11-solana-jc-chart-supply-bounds.md) дали43
timestamp-aligned price rows, но sparse gaps и same-second supply uncertainty
сохраняют unknown для максимума FDV. `timezone=UTC` в отдельном ответе
сдвинул epoch на−7200s; этот ответ не использован как price/supply history.

**Birdeye:** после добавления владельцем ключа пять валидных запросов OHLCV V3
ответили 200 во всех четырёх сетях. Два старых Base Zora токена вернули дневные
свечи начиная с октября 2025 года; годовое окно доступно на данном ключе.
Последующий запрос Codex `athPrice` позволил сравнить USD за токен
в одинаковых единицах: расхождения остались крупными и разнонаправленными.
Переключение Birdeye `outlier=true` для BSC/Robinhood не устранило их.
Для Solana отдельный `outlier=true` probe открыл пик в 1.004× Codex `athPrice`: фильтрация
объяснила этот sample. Base timeout сохранён, следующий отдельно разрешённый
запрос ответил 200; ранний более высокий peak всё равно отличается от Codex. Это требует проверки рынков и
индексации; Birdeye price high нельзя автоматически записывать как FDV. [Живой PoC](2026-10-11-birdeye-year-poc.md),
[OHLCV V3](https://data.birdeye.so/docs/data-api/price-ohlcv/get-defi-v3-ohlcv).
Free Standard документирует 30,000 CU/месяц; название плана локального ключа
по одному успешному ответу не установлено. [План](https://data.birdeye.so/docs/guides/payment/pricing).

**DexPaprika:** последующие публичные GET вернули недокументированные
`price_stats.ath` / `ath_date` во всех четырёх сетях, в том числе старше 180 дней.
Прежний вывод «нет поля в OpenAPI — нет direct ATH» оказался слишком сильным.
Проверки обнаружили крупные выбросы и расхождения; поле нельзя автоматически
записывать как ATH FDV. [Живые проверки и сравнение](2026-10-11-public-historical-ath-followup.md).

**CoinGecko core:** keyless contract lookup вернул price ATH для Base BRETT,
а старый Zora адрес — 404. Это полезное бесплатное дополнение для листингов,
отдельное от GeckoTerminal pool OHLCV. [Проверка](2026-10-11-listed-coin-ath-crosschecks.md).
GeckoTerminal подтверждён как keyless pool history за последние 180 дней.

Дополнительные кандидаты: [Flaunch](2026-10-11-base-launchpad-ath-research.md)
документирует public ATH/свечи, но host вернул 521; [Solana Tracker / OpenDEX](2026-10-11-solana-robinhood-ath-sources.md)
документируют direct ATH для отдельных сетей, но требуют ключ и проверки scope.
Биржевой ATH цены, pool `high` и token-wide ATH FDV не взаимозаменяемы.
Сохранённые Codex значения остаются отдельными наблюдениями, unknown не
превращается в ноль. Универсальное бесплатное lifetime ATH FDV покрытие
произвольных токенов пока не подтверждено; полезные частичные источники
проверяются отдельно по сетям и launchpads.

## Метрики нельзя смешивать

| Результат | Что он означает | Чего не доказывает |
|---|---|---|
| Цена ATH | Наибольшая цена, которую провайдер наблюдал для заданного рынка/токена | ATH FDV или максимум всех рынков токена |
| OHLCV `high` | Наибольшая цена внутри каждой свечи, агрегированная источником; максимум `high` по свечам покрывает внутрисвечные пики при корректной полноте свечи | Полноту пропущенных временных интервалов, других пулов или истории до начала индексации |
| Максимум, вычисленный приложением при периодическом запросе | Наивысшая цена среди собственных наблюдений с момента включения токена | Исторический ATH до первого наблюдения; обновления между опросами, если endpoint отдает только текущую цену |
| Codex `extrema.athFdv` | Поле, уже используемое существующим Codex-адаптером проекта | Автоматическую эквивалентность цене ATH, CEX ATH или `цена × текущий supply` |

Для сопоставления с Codex ATH FDV нужно установить выбранную supply-метрику и её временной срез по политике провайдера. Из просмотренных документов не следует, что поля ATH цены и ATH FDV в разных продуктах используют один знаменатель, временной срез supply или агрегацию пулов. Не преобразовывать историческую цену в `athFdv` без отдельной спецификации и проверки, какую supply и какой её временной срез использует ATH-метрика.

## Переиспользование уже имеющегося Base ATH

Read-only audit на 2026-10-11T00:59:55Z: 11,481,003 Base launches,
350,746 numeric ATH rows (350,205 positive, 541 numeric zero),
642,769 `codex-missing` unknown. Ordered merge join по PK `(chain,token)`
подтвердил, что все ATH rows связаны с сохранёнными Base launches.
Provider-index loader также пересекает результаты с `known_launch_tokens`
перед записью. Ранний audit не завершил эту связь; теперь она проверена.

154 закрытых `createdAt`-filter дня — степень продвижения отдельного
Codex discovery, а не возраст всех имеющихся ATH. В БД есть положительные
ATH для launches октября 2025 года. Три строки `codex-index-date-mismatch`
имеют numeric ATH и отдельный date-gap provenance. Эти данные сохраняются;
отсутствующие значения и непроверенное покрытие не подменяются нулями.
Точная диагностика: `.scout-data/diagnostics/base-ath-coverage-2026-10-11.json`.

## Сравнение провайдеров

Статусы ниже: `docs` — возможность явно есть в первичной документации; `probe` — ограниченно подтверждена публичным ответом; `unknown` — точная характеристика не установлена. `docs` и `probe` не равны доказательству полноты данных конкретного mint.

| Провайдер | Бесплатный путь и квота | История / точность | Сети проекта | Практическая оценка |
|---|---|---|---|---|
| **Birdeye Data API** | [План](https://data.birdeye.so/docs/guides/payment/pricing), [endpoint](https://data.birdeye.so/docs/data-api/price-ohlcv/get-defi-v3-ohlcv). `docs`: Standard $0, 30,000 CU/месяц, 1 запрос/с, ограниченный набор API. OHLCV V3 явно доступен на Standard. Требуются аккаунт и API key; OHLCV/intraday запросы на ключе владельца проверены, план этого ключа не установлен. | `docs`: `/defi/v3/ohlcv` принимает адрес токена, возвращает до 5,000 свечей; документация явно ограничивает хранение 1s свечей двумя неделями, 15s/30s — тремя месяцами. Для 1m и более крупных интервалов верхний возраст данных не указан. Не найден документированный all-time ATH endpoint: price-stats дает high только на интервалах до 7d. | `docs`: `solana`, `bsc`, `base`, `robinhood`; network table дает BSC chain ID 56 и Base 8453, но Solana и Robinhood помечены N/A. Robinhood в официальной странице сетей совпадает по имени и параметру с проектной публичной Robinhood Chain; сам Birdeye не указывает там `4663`. Testnet отдельным параметром не перечислен. | Лучший документированный all-four кандидат для bounded OHLCV PoC. `unknown`: полный охват каждого токена, длительность дневной истории, all-pool агрегация и сохранение истории при миграции ликвидности. |
| **Solana Tracker Data API** | [ATH endpoint](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price), Data API key via `x-api-key`. Owner account allowance:2,500 requests/month (2026-10-11), overriding the larger advertised allowance for budgeting. `probe`:3 ATH GETs HTTP200,1 TokenInfo and3 bounded chart GETs (7 total); no automatic retries. | Direct `highest_price`, timestamp and pool ID since provider recording began. Price kind, recording start per mint and all-pool/migration completeness remain unknown. Observed `highest_market_cap` matches peak price times later supply in these samples; not automatically project FDV. | Solana only; does not replace BSC/Base/Robinhood sources. | Key works, but all3 sampled FDV values remain unvalidated. Keep this7-call diagnostic separate from daemon cadence. |
| **GeckoTerminal Public API** | [Публичный API](https://api.geckoterminal.com/docs/index.html), [FAQ](https://apiguide.geckoterminal.com/faq). `docs`: публичный keyless API, без аккаунта. Документы расходятся: Swagger говорит приблизительно 10 запросов/мин с изменением под нагрузкой, FAQ — 30/мин. Для планирования брать более низкую цифру. | `probe`: старый запрос получил 401 с текстом об ограничении 180 дней; недавняя свеча того же SOL/USDC пула вернулась. Свеча включает поле high. `docs`: бесплатный OHLCV endpoint работает по адресу пула. Это доказывает 180-дневное окно на проверенном пуле, а не полную историю всех рынков. | `probe`: полный публичный `/networks` с pagination: `solana`, `bsc`, `base`, `robinhood`; RH на странице 3. Официальные RH docs различают mainnet 4663 и testnet 46630, имя сети совпадает. | Лучшее keyless дополнение/независимая проверка для последних 180 дней. `unknown`: поддержка каждого адреса и миграционного пути. |
| **CoinMarketCap DEX K-line** | `docs`: Keyless Public API явно документирует K-line candles без ключа/регистрации; keyless запросы используют общий IP rate pool, числовой лимит не задан. Для отдельного Basic key тарифная страница указывает 15,000 credits/месяц и 50 req/min. `probe`: keyless OHLCV запрос вернул HTTP 403, тело ошибки не сохранено. | `docs`: `/v1/k-line/candles` принимает platform ID/name, token/pool address, interval и `from`/`to`; response schema включает OHLCV и traders. `probe`: дневная Base WETH свеча за Oct 2025 была отклонена HTTP 403, так что фактический keyless доступ и история старше 180 дней не доказаны. `unknown`: retention/max candles. `pm=p` означает price, `pm=m` market-cap; семантика `m` к Codex ATH FDV не установлена. | `probe`: официальный keyless platform-list ответ содержит BSC id 14/chId 56, Solana id 16/chId -1, Base id 199/chId 8453, Robinhood Chain id 300/chId 4663; все `v=true`. Это подтверждает перечисление четырех mainnet platforms, но не отвечает на 403 OHLCV. | Сетевой каталог CMC покрывает четыре сети, однако candle endpoint требует отдельного решения по 403; на текущем evidence не считать работающей бесплатной OHLCV опорой. CMC v2 Price Performance ATH — другой listed-coin endpoint; текущий tutorial говорит Basic, authenticated доступ не проверен. |
| **Coinranking** | `docs`: Free key дает 5,000 запросов/месяц, 5 req/s burst, до 100 результатов. | Для монет, которые Coinranking уже знает, `/coins`/coin details возвращает `allTimeHigh.price` и timestamp; price history на Free ограничена одним годом. `unknown`: arbitrary mint coverage через контрактный фильтр. Полный `/blockchain/:blockchain/:address` contract lookup и tier 3 — не Free; contract-address UUID lookup указан для Professional. | `unknown`: arbitrary токены из Solana/BSC/Base/Robinhood. Coin addresses across chains описывают только coin mapping. | Подходит как легкий reference ATH для совпавших листингов; не является bulk resolver произвольной базы meme-token адресов. Не DEX OHLCV на бесплатном плане. |
| **CoinGecko onchain Pro / GeckoTerminal отдельно** | `docs`: платная CoinGecko Pro `/onchain` не равна независимому публичному GeckoTerminal API. | `docs`: Pro pool OHLCV — окно одного запроса до 6 месяцев, старше rolling 6 месяцев только Analyst+; tracking конкретного пула зависит от GeckoTerminal. `probe`: keyless 180d gate проверен отдельно. | `docs`: Pro changelog упоминает Solana, BNB и Base; Robinhood для Pro не проверялся. GeckoTerminal keyless: все четыре сети. | Не переносить платные условия `/onchain` на keyless поверхность и не считать ни один из режимов lifetime-ATH без отдельного подтверждения. |
| **CoinMarketCap Price Performance Stats** | [First-party tutorial 2026-08-04](https://coinmarketcap.com/api/resources/how-to-retrieve-price-performance-stats-roi-ath-atl/) указывает v2 endpoint на всех планах, включая Basic; прежнее утверждение о Startup не применять без сверки версии. Ключ требуется; live Basic не проверен. | `all_time.high` + timestamp для CMC-listed coin; принимает CMC ID, не произвольный DEX contract. | Coin-level coverage. | Selective cross-check; не универсальный resolver meme-token contracts. |
| **Moralis** | `docs`: текущая pricing page показывает Starter за $149/месяц при годовой оплате; бесплатного плана в текущей таблице нет. Старый официальный FAQ описывает бесплатный Starter 40,000 CU/day; страницы расходятся по времени актуальности. | `docs`: есть tutorial по историческим ERC-20 OHLC свечам, но бесплатный доступ и необходимая история не подтверждены. Profitability endpoint ограничивает учтенные swap события Uniswap V2/V3. | `unknown`: найденный ERC20 metadata enum содержит Solana/BSC/Base, но не Robinhood; это не доказывает coverage его price-history endpoints. | Не рекомендован как бесплатная all-four опора по текущей pricing page и найденным chain docs. |
| **Mobula direct ATH** | `docs`: pricing comparison показывает 10,000 бесплатных Demo credits. `probe`: no-auth запрос к `/api/2/token/ath` для известного Robinhood token вернул HTTP 403: API access недоступен на Free plan; Запрос с действующим бесплатным ключом не проверялся; рекомендовать free-доступ по этому ответу нельзя. | `docs`: официальный Swagger описывает endpoint; публичный запрос получил отказ по плану. Доступ с ключом Free/Demo не подтверждён. Это не свечная реконструкция. | Проверен один Robinhood token только для отказа по плану; all-four coverage не устанавливалось. | Прямой ATH endpoint существует, но бесплатный доступ не подтверждён; Robinhood не найден в полном публичном каталоге сетей Mobula. |
| **DexPaprika** | [Free pricing](https://dexpaprika.com/api/pricing): 100K rolling-30d credits с Free key, keyless лимит ниже; 30 rpm. | `probe`: public token GET отдаёт price-like `price_stats.ath` / date старше 180 дней. Поля отсутствуют в просмотренной OpenAPI схеме; обнаружены выбросы. Free pool OHLCV 7 дней; token OHLCV платный. | Шесть первоначальных token probes Base/SOL/BSC/RH — 200; дополнительные сравнения старых Base показали заметные расхождения. | Полезный reference/triage; metric semantics, pool selection и качество требуют проверки. Не писать поле как Codex ATH FDV. [Дополнение](2026-10-11-public-historical-ath-followup.md). |
| **Helius / DEX Screener (уже используемые факты)** | `docs`: DEX Screener имеет публичные latest-price endpoints с rate limits. Helius запрашивает API key. | `docs`: Helius DAS `getAsset` отдает текущую price information; [существующая project note](2026-10-10-base-ath-sources.md) фиксирует cache до 600s и заявленное top-10k покрытие по 24h volume. DEX Screener отдает `priceUsd`, FDV/marketCap, pair metadata; API reference не перечисляет historical candles/ATH. | Наличие текущих project adapters не устанавливает полный token-history DEX охват или RH ATH. | Исторический backfill этим не закрывается; текущие snapshots после старта polling нельзя обозначать ATH. |

### Расчет бесплатной емкости Birdeye

Документированная стоимость OHLCV V3 зависит от количества возвращенных свечей: 0–100 = 25 CU; 101–300 = 30 CU; 301–1,000 = 40 CU; 1,001–2,000 = 75 CU; 2,001–5,000 = 100 CU. Одна выборка на 365 дневных свечей попадает в диапазон 301–1,000, то есть 40 CU, если она возвращается одним ответом; 30,000 CU математически хватает на максимум 750 таких запросов за месяц до учета других endpoint и фактического поведения. При 5,000 свечах за запрос арифметический верх — 300 запросов/месяц. Это потолки по опубликованному счетчику, а не обещание полноты данных или реального числа требуемых запросов. Лимит 1 RPS означает, что эту емкость нельзя использовать как высокоскоростной fan-out.

## Сетевой охват: что подтверждено

| Сеть проекта | GeckoTerminal public `/networks` | Birdeye OHLCV V3 | CMC K-line |
|---|---|---|---|
| Solana mainnet | `solana`, direct public list | `solana`; supported-networks table | `Solana`, CMC platform id 16; `chId=-1` |
| BSC mainnet, 56 | `bsc` | `bsc`, ID 56 | BNB Smart Chain, CMC platform id 14; `chId=56` |
| Base mainnet, 8453 | `base` | `base`, ID 8453 | `Base`, CMC platform id 199; `chId=8453` |
| Robinhood Chain mainnet, 4663 | `robinhood`; официальный RH docs различает mainnet 4663 и testnet 46630 | `robinhood`; Birdeye table не указывает ID, official RH supports mainnet name; endpoint enum не показывает testnet | `Robinhood Chain`, CMC platform id 300; `chId=4663`; `v=true` |

Для GeckoTerminal страницы 1–3 публичного списка исчерпаны; страница 3 возвращает `next: null` и содержит `robinhood`. Для Birdeye endpoint перечисляет target parameters явно, но ее network table ставит N/A вместо chain ID у Solana/Robinhood. Сопоставление Birdeye `robinhood` с production chain 4663 убедительно по официальному chain name и тому, что в enum нет отдельного testnet selector, но exact ID не указан самой Birdeye. Ни один из этих фактов не подтверждает наличие полного покрытия токенов или конкретного пула.

CMC keyless platform-list response contains four project networks with `v=true`: BSC `id=14/chId=56`, Solana `id=16/chId=-1`, Base `id=199/chId=8453`, and Robinhood Chain `id=300/chId=4663`. CMC's own Solana `chId=-1` does not numerically identify mainnet, though the platform is named Solana. The RH entry exactly matches project mainnet ID 4663; testnet 46630 is not the entry returned.

## Ограниченная живая проверка GeckoTerminal

Проведено в рамках лимита четырех бесплатных публичных GET-запросов; без API key, аккаунта и заголовков авторизации:

1. `GET /api/v2/networks` — ответ содержит `solana`, `bsc`, `base` и pagination links; полный RH результат взят из прямой проверки sibling-агентом page 3: `robinhood`, `next: null`.
2. `GET /api/v2/networks/solana/pools/58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2/ohlcv/day?...before_timestamp=1759276800` — запрос свечи раньше бесплатного окна вернул JSON status 401 с сообщением об ограничении Public API данными последних 180 дней и требованием Analyst+ для глубже.
3. Повтор той же старой свечи с User-Agent вернул то же явное ограничение; это диагностический повтор, не доступ к защищенной данным.
4. Тот же пул с `before_timestamp=1791676800`, limit 1, вернул daily OHLCV свечу с timestamp 1791590400 и полями open/high/low/close/volume.

Прямые ссылки на публичные ответы: [network list, page 1](https://api.geckoterminal.com/api/v2/networks), [network list, page 3](https://api.geckoterminal.com/api/v2/networks?page=3), [старая SOL свеча, отказ по 180 дням](https://api.geckoterminal.com/api/v2/networks/solana/pools/58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2/ohlcv/day?aggregate=1&limit=1&before_timestamp=1759276800&currency=usd), [свежая SOL свеча](https://api.geckoterminal.com/api/v2/networks/solana/pools/58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2/ohlcv/day?aggregate=1&limit=1&before_timestamp=1791676800&currency=usd). Live probes служат узким доказательством поведения одного известного SOL/USDC пула и заявленного возрастного gate; они не измеряют пуловую полноту или надежность сети.

## Ограниченная keyless проверка CoinMarketCap

Official docs specify `https://pro-api.coinmarketcap.com/public-api` as the keyless base, state that requests use GET without API key/headers, and document both `/v1/dex/platform/list` and `/v1/k-line/candles` as keyless endpoints. Platform list was fetched twice; both calls returned HTTP 200 with identical 65,517-byte JSON, 244 platform records, and `status.error_code=0`. Target records from the response were `{"id":14,"n":"BNB Smart Chain (BEP20)","chId":56,"pltA":"BSC","v":true}`, `{"id":16,"n":"Solana","chId":-1,"pltA":"SOL","v":true}`, `{"id":199,"n":"Base","chId":8453,"pltA":"BASE","v":true}`, and `{"id":300,"n":"Robinhood Chain","chId":4663,"pltA":"","v":true}`. This proves current platform-directory entries, not that arbitrary token/pool addresses have candles.

One keyless candle request followed the official OHLCV path and parameters. Exact request URL: `https://pro-api.coinmarketcap.com/public-api/v1/k-line/candles?platform=199&address=0x4200000000000000000000000000000000000006&interval=1d&from=1759276800&to=1759363200&unit=usd&limit=1&pm=p`. It asked for one daily price candle for Base WETH on 2025-10-01 UTC, older than 180 days at research time. Response was HTTP 403; the client did not capture the response body. The reason (plan gate, edge policy, or another refusal) is unknown, so this does not establish candle availability or retention depth. Do not retry or interpret it as an ATH result.

## Кандидаты, которые не закрывают задачу самостоятельно

- **DEX Screener:** официальный API документирует быстрый current snapshot: token batch до 30 адресов, `priceUsd`, `fdv`, `marketCap`, `pairCreatedAt`; исторические OHLCV/ATH endpoints в reference не перечислены. Использовать как current corroboration, не как history.
- **Helius DAS:** `getAsset` описывает текущую price information, не историю. В существующей заметке проекта указано покрытие top 10k по 24h volume и cache до 600s. Это полезно для live valuation, но ни неизвестные адреса, ни пропущенные исторические пики не восстанавливает.
- **Coinranking:** бесплатный ATH уже есть для Coinranking-listed coin, но free tier ограничивает историю одним годом/100 точками; UUID lookup по контракту — Professional, а free tier excludes Tier 3. Это подходит для cross-check популярных listed coins, не arbitrary memecoin database.
- **CMC core ATH:** first-party tutorial для v2 говорит о доступе Basic; это listed-coin endpoint с CMC ID, live entitlement не проверен. Это не следует смешивать с keyless `/v1/k-line/candles`, который относится к DEX адресам и является отдельной потенциальной свечной стратегией.
- **Moralis:** текущая pricing page не показывает free tier; старая official FAQ говорит обратное. В найденной profitability модели только swap topics Uniswap V2/V3 считаются. Даже при историческом API этот путь не доказан как бесплатный общесетевой ATH для произвольных Solana/BSC/Base/RH адресов.
- **Mobula:** `/api/2/token/ath` — реальный direct ATH endpoint, но публичный запрос без ключа вернул HTTP 403 с текстом, что API access недоступен на Free plan; 10,000 Demo credits не означают доступ к этому endpoint.
- **DexPaprika:** public token responses содержат ATH, несмотря на отсутствие полей в просмотренной схеме; outliers и расхождения требуют проверки. Free pool OHLCV — 7 дней, token OHLCV платный.

## Дополнительные первичные источники (короткая проверка)

- **GoldRush/Covalent — лучший дополнительный PoC, но временный и только 3/4 OHLCV-сетей.** Beta `OHLCV Tokens Query` сейчас документирован как без списания API credits, имеет `interval` + `timeframe`, отдаёт candle `high` и принимает до 5 token addresses за query. Его endpoint enum перечисляет Solana, BSC и Base, но не Robinhood. Ответ token query даёт `pair_address` «primary DEX pool with the most liquidity», не доказанную агрегацию всех рынков. Весь GoldRush API free tier — 14 дней, 25,000 credits, затем free access не заявлен для постоянной работы; general chain directory действительно содержит Robinhood, но это не доказывает OHLCV endpoint support для сети. Максимум документации для сроков OHLCV — `timeframe` enum без перечисленных границ, поэтому history depth остаётся unknown.
- **DeFiLlama Coins API — дополнительная sampled-price проверка, не candle ATH.** Официальный free/Open plan включает prices; official SDK/API описывает `/chart/{coins}` как цены в регулярных интервалах и принимает идентификатор вида `chain:address`. Это позволяет оценить максимум точек только для активов, у которых DeFiLlama price API выдаёт историю; это не OHLCV `high`, произвольное покрытие четырёх сетей и lifetime полнота не подтверждены. RH-specific price API mapping не найден.
- **DEXTools API V3 — free key есть, ready ATH/candle route не найден.** Free tier: 10,000 credits/month, 60 req/min, обязательны account/API key; reference token/pool metrics показывает current price и изменения до 24h, но не ATH и не candle highs. FAQ прямо перечисляет Solana, BNB и Base, а RH не подтверждён; полный network list сам требует key. Не использовать показанный max/latest metric как ATH.
- **DexGuru — ограниченный EVM candidate, не свечной источник для требуемых сетей.** Official API требует key; free personal quota заявлена как 200,000 calls/month, 5 RPS и 300-second update delay. Docs перечисляют старые EVM сети (в т.ч. BSC), не Solana/Base/RH, и предупреждают, что historical charts/aggregates начинаются с внедрения конкретного DEX/AMM. В найденной official API документации не подтверждён ready all-time ATH или candle-high endpoint.
- **Bitquery — есть сеть и торговая история, но free trial не включает archive.** Актуальная pricing page ограничивает бесплатный trial 7 днями realtime и 1,000 API points; для BSC/Base/RH и Solana OHLCV/token-price history нужен отдельный paid per-chain archive add-on. Paid archive доходит до начала индексации Bitquery, не автоматически до рождения любого token.
- **The Graph — бесплатный query allowance, но это не готовая DEX price API.** Docs дают 100,000 free queries/month для опубликованных subgraphs; данные и схема зависят от конкретного subgraph, поэтому универсального токен-адресного свечного ATH нет. Robinhood Chain (`eip155:4663`) в supported networks включён как Substreams/Extended EVM, тогда как subgraphs column для него пустая; нельзя переносить RH chain-level streaming support на готовые DEX subgraphs или candles.

Из этого набора GoldRush заслуживает отдельного маленького PoC для новых/недавно запущенных токенов Solana/BSC/Base, если 14-day trial подходит как одноразовая оценка. Он не заменяет Codex lifetime anchors и не закрывает Robinhood; DeFiLlama sampled points разумны лишь для cross-check. DEXTools/DexGuru/Bitquery/The Graph не дают подтверждённой free all-four historical ATH реализации по найденным официальным условиям.

## Рекомендованный bounded PoC

Не делать полный проход по миллионам строк. Сначала выбрать малую фиксированную выборку, например 10 адресов на сеть, где для каждого известны: корректный mainnet address, дата создания/первой пары, текущий Codex ATH и хотя бы один известный пул; добавить несколько бывших/migrated и без активной ликвидности адресов. Не включать credentials в логи.

1. На Birdeye free Standard один раз запросить 365 дневных свечей, затем более длинный count-mode query для каждого адреса там, где документация/ответ допускает. Сохранить requested interval, фактический timestamp первой и последней свечи, число свечей, ошибки/N/A, provider и network IDs. Не считать первую возвращенную свечу запуском токена, пока не сверена с известной launch date.
2. Для токенов, где Birdeye возвращает данные, сравнить price `max(high)`, timestamp, текущий price, Codex `athFdv` и независимый известный пул. Отдельно спросить pair OHLCV / топ-пулы, чтобы установить, token-level series агрегирует ли пулы или следует одному источнику; проверить migration source и destination не объединяя их молча.
3. Для GeckoTerminal на тех же активных/старых пулах сравнить `max(high)` последних 180 дней, произвести один запрос старше 180 дней и записать expected 401. Не делать фоновый массовый pool discovery через rate limit около 10–30 rpm.
4. До решения о внедрении ограничить budget заранее: Birdeye 30,000 CU/месяц и 1 RPS; отдельный quote budget не применять. Посчитать фактические CU по счетчикам из response/dashboard и фактический адресный набор. Первый live Birdeye PoC на предоставленном ключе выполнен; дальше нужны representative coverage/price-policy проверки из нового отчёта.

Ожидаемый результат PoC — не «нашли provider ATH у всех», а таблица coverage: настоящий ответ / отсутствие индекса / история начинается позже launch / address unsupported / временной hole / pool migration / exact disagreement. Поддержка всех четырех сетей служит фильтром кандидата, но не coverage-gate для отдельного адреса.

## Проверенные первичные источники

- [GeckoTerminal Public API docs and Swagger](https://api.geckoterminal.com/docs/index.html) — endpoints, cache, публичный rate note и network listing.
- [GeckoTerminal API guide](https://apiguide.geckoterminal.com/) и [FAQ](https://apiguide.geckoterminal.com/faq) — бесплатный keyless доступ, OHLCV требует pool address, частота запросов; API находится в beta.
- [CoinGecko Pro OHLCV changelog](https://docs.coingecko.com/changelog/10122018) — полугодовые окна запросов, более старая история для Analyst+ и tracking-dependent availability.
- [Birdeye OHLCV V3](https://data.birdeye.so/docs/data-api/price-ohlcv/get-defi-v3-ohlcv), [Price Historical](https://data.birdeye.so/docs/data-api/price-ohlcv/get-defi-history-price), [Price Stats](https://data.birdeye.so/docs/data-api/stats/get-defi-v3-price-stats-single) — endpoint, plan accessibility, candle limit, data age for fine-grain intervals, CU schedule, timeframe highs.
- [Birdeye pricing](https://data.birdeye.so/docs/guides/payment/pricing), [endpoint access by package](https://data.birdeye.so/docs/guides/data-accessibility-by-packages), [supported networks](https://data.birdeye.so/docs/resources/data-coverage/supported-networks), [DEX coverage](https://data.birdeye.so/docs/resources/data-coverage/dex-coverage), [Robinhood release](https://data.birdeye.so/docs/changelog/20260709-release-robinhood-chain-support).
- [Robinhood Chain network connection docs](https://docs.robinhood.com/chain/connecting/) — public Robinhood Chain mainnet chain ID 4663 and testnet 46630.
- [CoinMarketCap Keyless Public API](https://coinmarketcap.com/api/documentation/pro-api-reference/keyless-public-api), [DEX K-line candles](https://coinmarketcap.com/api/documentation/pro-api-reference/ohlcv/), [platform list](https://coinmarketcap.com/api/documentation/pro-api-reference/platform), [coin Price Performance Stats](https://coinmarketcap.com/api/documentation/pro-api-reference/cryptocurrency). Probed responses: [keyless platform list](https://pro-api.coinmarketcap.com/public-api/v1/dex/platform/list) and [Base WETH Oct 1 2025 candle request](https://pro-api.coinmarketcap.com/public-api/v1/k-line/candles?platform=199&address=0x4200000000000000000000000000000000000006&interval=1d&from=1759276800&to=1759363200&unit=usd&limit=1&pm=p), which returned HTTP 403 (body not captured).
- [CoinMarketCap pricing](https://coinmarketcap.com/api/pricing/) — free Basic credits; per-endpoint access still governed by endpoint plan table.
- [Coinranking free API rate limits](https://coinranking.com/api/documentation/rate-limit), [coins endpoint](https://coinranking.com/api/documentation/coins/coins), [historical crypto data](https://coinranking.com/api/features/historical-crypto-data), [contract metadata and lookup plans](https://coinranking.com/api/features/coin-metadata-api).
- [Moralis current pricing](https://moralis.com/pricing/), [older free Starter FAQ](https://developers.moralis.com/faq/what-are-the-limits-for-api-requests/), [historical ERC-20 OHLC tutorial](https://docs.moralis.com/get-started/tutorials/data-api/prices-and-charts/get-historical-erc-20-token-prices-ohlc-data), [PnL pricing/swap limits](https://docs.moralis.com/web3-data-api/evm/reference/wallet-api/get-wallet-profitability).
- [Mobula API Swagger](https://api.mobula.io/docs), [Mobula plan comparison](https://mobula.io/vs-coingecko) — direct ATH endpoint and Free/Demo plan distinction.
- [DexPaprika token latest response](https://docs.dexpaprika.com/api-reference/tokens/get-a-tokens-latest-data), [DexPaprika pricing](https://dexpaprika.com/api/pricing) — latest-token fields and free/paid OHLCV limits.
- [GoldRush OHLCV Tokens Query](https://goldrush.dev/docs/api-reference/streaming-api/queries/ohlcv-tokens-query), [endpoint reference](https://goldrush.dev/docs/skills/goldrush-streaming-api/references/endpoints/), [pricing](https://goldrush.dev/pricing/), [network directory](https://goldrush.dev/chains/) — beta token candle high, supported-chain enum, temporary trial allowance and separate general chain coverage.
- [DeFiLlama API pricing](https://docs.llama.fi/pro-api), [official API docs](https://api-docs.defillama.com/), [official API SDK price examples](https://github.com/DefiLlama/api-sdk) — free prices plan and regular-interval price chart interface.
- [DEXTools API V3 docs](https://api-portal-dev.dextools.io/docs), [token metrics](https://api-portal-dev.dextools.io/docs/tokens/TokensV3Controller_getTokenMetrics), [FAQ](https://api-portal-dev.dextools.io/docs/faq) — required API key, 10k free credits, available price fields and documented chains.
- [DexGuru API start and free tier](https://dexguru.readme.io/docs/getting-started), [data completeness](https://docs.dex.guru/data/data-completeness), [data FAQ](https://docs.dex.guru/general/faq) — free personal limits, chain list and DEX integration date cutoff.
- [Bitquery pricing](https://bitquery.io/pricing), [paid archive-plan documentation](https://docs.bitquery.io/docs/ide/paid/), [Robinhood network API docs](https://docs.bitquery.io/docs/blockchain/robinhood/) — trial/history distinction and per-chain historical datasets.
- [The Graph subgraph query docs](https://thegraph.com/docs/en/subgraphs/overview/), [supported networks](https://thegraph.com/docs/en/supported-networks/), [query introduction](https://thegraph.com/docs/en/subgraphs/querying/introduction/) — query free allowance, per-subgraph schemas, and Robinhood Substreams support.
- [DEX Screener API reference](https://docs.dexscreener.com/api/reference), [Helius DAS getAsset](https://www.helius.dev/docs/api-reference/das/getasset).

## Файлы, проверки и ограничения исследования

Продолжение 2026-10-11: найден keyless [StockKit Market API](https://docs.stockkit.dev/products/market-api/)
для Robinhood stock tokens. `/v1/stats/NVDA` связывает инструмент с точным
контрактом на4663; `/v1/history/NVDA?interval=5m&limit=300` возвращает
5-minute samples NVDA/USDG из выбранного Uniswap v3 пула. Проверенный
index history начинается2026-09-16, поэтому August26 этот API не покрывает;
USDG quote также требует независимого FX, а samples могут пропустить пик.
Это дополнительный источник для stock quote, не ATH API всех meme-токенов.

Для August26 удалось проверить уже существовавший пул через archive RPC,
успешные receipts ближайших сделок и verified runtime; dated traded
[Kraken USDG/USD](https://api.kraken.com/0/public/OHLC?pair=USDGUSD&interval=1440&since=1787616000)
после exact Paxos/RH contract-identity проверки подтвердил SMOGGY3 USD level.
Исторические [Chainlink rounds](https://reference-data-directory.vercel.app/feeds-robinhood-mainnet.json)
также доступны бесплатно, но проверенные marks оказались слишком старыми
и в этом FDV не использованы. Details, request budgets и SHA сохранены в
[Robinhood audit](2026-10-11-robinhood-ath-supply-source-audit.md).
Ни один из этих результатов не доказывает полное годовое/lifetime покрытие.

- Новый файл: `docs/p0/measurements/2026-10-11-free-ath-providers.md`.
- Проверены official provider docs/pricing/network lists; проведено четыре unauthenticated public GeckoTerminal GET probes для network list и SOL/USDC OHLCV, sibling agent отдельно проверил последнюю страницу `/networks`; один no-auth Mobula request подтвердил отказ Free ATH endpoint; CMC: две keyless platform-list GET (дубликат для разбора уже полученного body) и одна old-candle GET (403). Дополнительный pass по DEXTools, DexGuru, DeFiLlama, GoldRush, Bitquery и The Graph использовал только official docs/pricing; их API не вызывались. Это перечень первого прохода: позже Birdeye, DexPaprika, Coinranking и CoinGecko core проверены живыми запросами; новые отчёты выше фиксируют requests и ограничения.
- Этот research pass не менял ATH, код или задачи. Последующие Codex backfill jobs описаны отдельно в отчёте результатов; их расход не входит в provider probes. Исследование не измеряет production latency, coverage всех адресов или права распространения данных.
- Открытые проверки: Birdeye daily-history completeness, pool/indexing coverage, причины same-unit ATH расхождений и historical supply; фактический mainnet chain ID binding Birdeye `robinhood`; GeckoTerminal coverage multi-pools/migrations per token; CMC OHLCV 403 cause, actual keyless access, and retention; GoldRush trial OHLCV output, timeframe boundary, and RH endpoint support; конкретная coverage любого API у токенов без индексированного пула. Mobula plan failure проверен на одном RH token, а не на всех сетях.
- Следующий шаг: bounded проверка конкретных пулов и lifecycle на предоставленном Birdeye ключе; первый year-window PoC и outlier follow-up уже выполнены. До подтверждения units/coverage/provider policy не писать свечные maxima в ATH FDV. Null/unsupported сохранять как coverage gap.
