# Бесплатные ATH-источники для Base и Robinhood Chain: Moralis и launchpad API — 2026-10-11

## Вывод

Подтверждённого бесплатного источника, который закрывает исторический ATH оставшихся runner-candidate адресов Base и Robinhood Chain, не найдено. Moralis — содержательный кандидат для платной реконструкции цены по DEX-парам, но не бесплатный: текущая официальная цена Starter составляет $149/месяц при годовой оплате, а endpoint требует `X-API-Key`. Старые упоминания бесплатного плана не соответствуют текущей опубликованной тарифной странице. Для Base у Moralis документирован pair OHLCV; для Robinhood Chain Moralis прямо ограничивает поддержку Universal API, тогда как OHLC endpoint содержит конечный EVM chain enum без RH/4663. Robinhood-only current API нельзя считать историческим ATH.

Из просмотренных launchpad API Four.Meme даёт текущую цену/торговые данные для своего BSC-подмножества, но не исторические свечи или ATH. Flaunch уже является документированным потенциальным Base-подмножеством в отдельном исследовании, однако live host на момент двух проб возвращал 521; это не решение для Robinhood и не подтверждённый работающий источник. Для RH прямого first-party токенового ATH/OHLC API в проверенных материалах не установлено.

## Moralis: точное соответствие задаче

| Свойство | Проверенный факт | Что это значит для ATH |
|---|---|---|
| Текущий бесплатный план | Текущая официальная страница перечисляет Starter за **$149/месяц при годовой оплате**, с 2 млн CU/месяц, 40 RPS, Price API и доступом ко всем сетям. На странице нет бесплатного Data API плана. | Считать Moralis бесплатным источником по опубликованным текущим тарифам нельзя. Старые ссылки на бесплатный Starter устарели/конфликтуют с текущей страницей. |
| API auth | EVM Price API пример требует `X-API-Key`; документы «Get your API key» предполагают аккаунт/ключ. | В этом проходе ключ или API-вызов не использовались. |
| Историческая цена токена | `GET /api/v2.2/erc20/{address}/price` описан как текущая или историческая цена в native и USD; есть `to_block`, а ответ включает USD price и `toBlock`. Стоимость по таблице CU — 50 CU за EVM token price. | Точечные historical price запросы не дают сами по себе максимума без полного набора наблюдений/блоков и правил выбора пула. |
| OHLC | `GET /api/v2.2/pairs/{address}/ohlcv` принимает **pair address**, `chain`, `timeframe` (`1s`, `10s`, `30s`, `1min`, `5min`, `10min`, `30min`, `1h`, `4h`, `12h`, `1d`, `1w`, `1M`), `currency=usd|native`, обязательные `fromDate`/`toDate` либо block bounds, `limit` и `cursor`. Ответ содержит `timestamp/open/high/low/close/volume/trades`. Стоимость — 150 CU за свечной запрос. | Candle `high` пригоден для реконструкции максимума **только по конкретным полученным парам и интервалу**; это не прямой ATH contract. `fromDate`/`toDate` + cursor позволяют запрашивать даты, но в просмотренной документации не найден гарантированный retention/depth SLA или максимальная страница. Следовательно, lifetime coverage и старт индекса остаются неизвестны. |
| Поиск пар / охват миграций | OHLC endpoint ссылается на Token Pairs endpoint для нахождения pair addresses. Marketing утверждает, что отслеживаются новые монеты и пулы на DEX; docs не обещают полноту всех исторических пулов адреса, переходов bonding-curve→DEX или последующих миграций. | Даже при платном доступе нужен PoC на токенах с несколькими пулами/миграцией и проверка пагинации/самой ранней свечи. Нельзя считать пару из списка единственной. |
| Base | Токен Price и OHLC endpoint enum явно содержат `base` и `0x2105` (mainnet); это подтверждённая поддержка этих API endpoints. | Технически пригодный paid candidate для Base-pool-history reconstruction; полнота пула/launchpad-coverage не доказана. |
| Robinhood Chain | Supported Chains docs говорят: **Robinhood Chain currently supported through Universal API only**. EVM token price/OHLC `chain` enum включает Base, BSC и другие коды, но не `robinhood` или `4663`. | RH chain support нельзя переносить с Universal API на EVM OHLC. В проверенной документации не найден RH historical price/OHLC endpoint; считать Moralis источником RH ATH нельзя. |
| Прямой ATH / FDV | В изученных Price API responses/docs нет токенового поля all-time-high. | Не найден direct ATH endpoint. Свечной максимум цены не равен историческому FDV; supply denominator и историческая полнота рынков остаются отдельными условиями. |

Официальные источники: [текущие Moralis plans](https://moralis.com/pricing/), [Moralis EVM token price](https://docs.moralis.com/data-api/evm/price/token-price), [OHLC by pair address](https://docs.moralis.com/data-api/evm/price/ohlc), [стоимость Data API в CU](https://docs.moralis.com/data-api/pricing), [supported chains и ограничение RH Universal-only](https://docs.moralis.com/data-api/supported-chains), [получение API key](https://docs.moralis.com/get-started/get-your-api-key).

На текущем тарифе 2 млн CU дают теоретически до 13,333 OHLC endpoint calls по 150 CU, если весь бюджет отдать только свечам; это верхняя арифметическая граница, не рекомендация и не план полного ATH-backfill. Реально цена аккаунта и другие запросы/ограничения уменьшают доступный объём. Для 59 Base + 30 RH cohort задача всё равно упирается в RH endpoint coverage, а не только в квоту.

## First-party launchpad paths

### Four.Meme (контрольный пример, BSC only)

Официальный integration repository описывает read-only token info через `TokenManagerHelper3.getTokenInfo`: версия, manager, quote asset, `lastPrice`, offers и статус добавления ликвидности. Публичный REST token list/rankings предоставляет текущие/короткие ranking окна; новые токены создаются на BSC V2. Прямой ATH/документированная историческая OHLC API в reviewed endpoint inventory отсутствует. Значит даже сильный источник live для launchpad-токенов не решает исторический ATH без собственного непрерывного сбора.

Источники: [Four.Meme token creation/integration docs](https://github.com/four-meme-community/fourmeme-docs/blob/main/docs/create-guide.md), [trade guide и `getTokenInfo`/`lastPrice`](https://github.com/four-meme-community/fourmeme-docs/blob/main/docs/trade-guide.md), [официальный Four.Meme integration skill/API summary](https://github.com/four-meme-community/four-meme-ai/blob/main/skills/four-meme-integration/SKILL.md). Его API BSC-only, поэтому не покрывает Base/RH.

### Flaunch (Base only; существующая частичная возможность)

В ранее выполненном [Base launchpad research](2026-10-11-base-launchpad-ath-research.md) first-party Flaunch REST schema содержит `price.allTimeHigh`, OHLC high-массивы и `meta.timeRanges`; официальная документация говорит о публичном no-auth доступе. Но два documented-host GET получили HTTP 521. ATH currency/supply semantics, retention, all-pool/migration coverage не установлены. Это требует отдельного повторного availability/semantics PoC перед использованием и не относится к RH.

### Robinhood/Pons

В просмотренных материалах нет first-party Pons token ATH или OHLC endpoint, пригодного как свободный архив. Pons launch/migration контракты дают возможность собственному индексатору восстанавливать onchain trades, но это уже отдельная ingestion-реализация с проверкой version/factory/pool lineage, USD conversion и supply; это не готовый free provider. Отсутствие найденного endpoint не доказывает, что у команды нет непубличного сервиса.

## Ограниченное продолжение

1. Не заводить Moralis как «free». Если владелец отдельно рассматривает бюджет, сначала проверить фактический план/права в аккаунте и провести маленький оплачиваемо-ограниченный PoC только на Base; ключ в чат не передавать. Проверить несколько исторических пар одного токена, cursor, timestamps, high и покрытие после migration. Документы не гарантируют retention.
2. RH проверять отдельным провайдером: у Moralis пока не подтверждён endpoint RH OHLC; наличие RH в Universal API не означает наличие исторической свечной поверхности.
3. Flaunch можно перепроверить только после подтверждения восстановления публичного host; его `allTimeHigh` и OHLC для своего indexed subset всё ещё требуют price-vs-marketcap, denominator, pair/migration и lifetime-window validation.
4. Не заменять отсутствующий ATH текущим market cap, коротким ranking high или свечой последнего найденного пула. Любой импорт ограничить доказанными адресами/парами и хранить начало окна/coverage gaps.

## Действия и ограничения

- Использованы официальные docs, first-party pricing/API references и repository docs для Four.Meme; внешние API данных не вызывались.
- Локальная проверка конфигурации читала только наличие четырёх заранее заданных переменных (`MORALIS_API_KEY`, `SCOUT_MORALIS_API_KEY`, `DEXTOOLS_API_KEY`, `SCOUT_DEXTOOLS_API_KEY`); все четыре не настроены в `.env`. Ни значение, ни имя других переменных не выводились.
- Cohort `59 Base + 30 Robinhood` принят как заданный scope; эта заметка не пересчитывает eligible адреса и не меняет ATH/FDV в БД.
- Результат подтверждает пригодность документации/планов, не полноту индексируемых токенов или live endpoint availability. Следующий конкретный unresolved факт — есть ли у Robinhood Chain провайдер с бесплатным history endpoint и подтверждённой retention; по Moralis ответ для documented EVM OHLC API сейчас отрицательный.
