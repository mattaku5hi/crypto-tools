# Listed-coin direct ATH cross-check: CoinGecko core API — 2026-10-11

## Вывод

CoinGecko core API даёт useful, уже вычисленный direct ATH price/date для токенов, чьи контракты разрешаются в CoinGecko coin page. Это отдельный путь от GeckoTerminal on-chain pools/OHLCV: успешный Base BRETT lookup вернул `market_data.ath.usd` и `market_data.ath_date.usd`; один другой Base contract lookup получил HTTP 404. Это подходящий адресный cross-check/заполнение для CoinGecko-listed токенов, но не универсальный mint ATH feed: отсутствие CoinGecko coin entry остаётся gap.

Публичный keyless вызов проверен без account/API key. Официальный pricing разделяет его с зарегистрированным Demo plan: keyless Public API — ориентировочно 5–15 calls/min в зависимости от нагрузки; бесплатный Demo plan даёт 10,000 calls/month и 100 calls/min, но требует аккаунта и API key. Успешная keyless проверка не доказывает, что endpoints всегда одинаково доступны без ключа или имеют стабильную месячную квоту.

## Endpoint, network IDs и ответы

Документированный core endpoint:

```text
GET https://api.coingecko.com/api/v3/coins/{asset_platform_id}/contract/{contract_address}
```

В [CoinGecko API reference/changelog](https://docs.coingecko.com/changelog/10122018) contract lookup возвращает CoinGecko token data; `market_data` включает ATH-related поля. Официальная документация также перечисляет исторические endpoint варианты `.../market_chart` и `.../market_chart/range`, но этот отчёт использует именно готовые поля ATH от token metadata endpoint.

Keyless `GET /api/v3/asset_platforms` вернул эти строки (11 октября 2026):

| ID | chain_identifier | Имя | Связь с требуемой сетью |
|---|---:|---|---|
| `base` | 8453 | Base | Base mainnet |
| `binance-smart-chain` | 56 | BNB Smart Chain | BSC mainnet |
| `solana` | `null` | Solana | Solana platform; API не выдаёт EVM-style integer chain ID |
| `robinhood` | 4663 | Robinhood | Совпадает с публичной Robinhood Chain mainnet ID 4663; это не Robinhood brokerage app и не testnet 46630 |

Эта платформенная directory подтверждает поддерживаемые identifiers, но не доказывает, что CoinGecko имеет coin page для каждого token contract в сети.

**Положительная проверка — BRETT/Base.** Запрос без ключа [к exact Base contract address](https://api.coingecko.com/api/v3/coins/base/contract/0x532f27101965dd16442e59d40670faf5ebb142e4) получил HTTP 200 и coin id `based-brett`, `asset_platform_id=base`, совпавший `contract_address`. В повторе с корректно разделёнными query parameters ответ был 28,142 bytes и включал:

| Поле | Значение ответа |
|---|---|
| `market_data.current_price.usd` | `0.00587861` |
| `market_data.ath.usd` | `0.234204` |
| `market_data.ath_date.usd` | `2024-12-01T08:31:18.000Z` |
| `market_data.ath_change_percentage.usd` | `-97.48996` |
| `market_data.market_cap.usd` | `58,227,723` |
| `market_data.total_supply` | `9,909,600,389.700281` |
| `last_updated` | `2026-10-11T01:12:53.000Z` |

Это `coin-page ATH price`, не доказательство ATH FDV и не сырая цена каждого отдельного AMM pool. Полученные рядом current market cap / total supply не являются автоматически denominator и supply-at-ATH для отдельного `athFdv`.

**Отрицательная проверка — другой Base contract.** Keyless GET по указанному старому Base Zora contract `0xa187403adcc6fcdf64fe9c850e0d9faf4ba3837f` получил HTTP 404 и не вернул coin object. Это доказывает только, что именно этот адрес не разрешился в tested CoinGecko core contract route в момент запроса; это не говорит ничего о каждой другой сети/монете и не следует трактовать как отсутствие токена on-chain.

## Планы и пределы scope

| Режим | Официально опубликованные условия | Что подтверждено здесь |
|---|---|---|
| CoinGecko Keyless Public API | [Pricing page](https://www.coingecko.com/en/api/pricing) предлагает «Keyless Public API»; [support article](https://support.coingecko.com/hc/en-us/articles/4538771776153-What-is-the-rate-limit-for-CoinGecko-API-public-plan) говорит 5–15 calls/min в зависимости от global usage; фиксированная месячная квота для keyless режима в этой документации не задана. | Пять небольших public GET выполнены без API key; выбранные platform directory и Base BRETT contract route ответили 200, Zora contract route — 404. Конкретно этот endpoint path работал без ключа для положительного примера. |
| CoinGecko Demo API | [Pricing page](https://www.coingecko.com/en/api/pricing): $0, 10,000 call credits/month и 100 calls/min. [Official signup instructions](https://support.coingecko.com/hc/en-us/articles/21880397454233-User-Guide-How-to-sign-up-for-CoinGecko-Demo-API-and-generate-an-API-key) требует регистрацию и ключ. | Не создавались аккаунт или API key; Demo plan не проверялся. Не переносить его гарантии на keyless Public mode. |

CoinGecko coin-level `ath`/`ath_date` — провайдерская агрегированная историческая метрика для resolved/listed coin, а не простой `max(high)` из специально выбранного пул-ряда. История ограничена тем, что CoinGecko листил/наблюдал и сохранил для coin page. В official changelog отдельно сказано, что для отделённых новых non-native/bridged assets исторические данные доступны лишь начиная с создания asset page; этот caveat важен для completeness. Здесь нет доказательства полного coverage launchpads, внешних рынков, pool migrations или CoinGecko unlisted coins.

## Практическая роль и PoC

Для shortlist/corroboration можно пробовать core address lookup после разрешения exact network `asset_platform_id`; принимать `ath` только если совпали возвращённые platform ID и contract address. Unknown/404 должны оставаться coverage gap, не нулевой ATH. CoinGecko-listed address может дать моментальный независимый ATH price/date без перебора candles. Для runner selection следует затем сравнить его с Codex `athFdv` и определением проекта, не подменяя одно другим.

В bounded follow-up (только если нужно) достаточно взять малую фиксированную выборку известных CoinGecko-listed и unlisted контрактов по каждой сети, сравнить ATH timestamps/values с текущим источником и классифицировать 200/404. Для Robinhood пока подтверждены asset platform ID 4663, но contract-level RH lookup не делался; readiness для каждой монеты остаётся unknown.

## Источники и выполненные действия

- [CoinGecko Coin Data by Token Address / API changelog](https://docs.coingecko.com/changelog/10122018) — core contract-address lookup и исторические product changes.
- [CoinGecko plan pricing](https://www.coingecko.com/en/api/pricing) и [public-plan rate limit FAQ](https://support.coingecko.com/hc/en-us/articles/4538771776153-What-is-the-rate-limit-for-CoinGecko-API-public-plan) — разделение free Demo и keyless Public usage/rate limit.
- [CoinGecko Demo key setup](https://support.coingecko.com/hc/en-us/articles/21880397454233-User-Guide-to-sign-up-for-CoinGecko-Demo-API-and-generate-an-API-key) — account/API key requirement for Demo.
- [Probe: asset platform list](https://api.coingecko.com/api/v3/asset_platforms); [probe: BRETT contract lookup](https://api.coingecko.com/api/v3/coins/base/contract/0x532f27101965dd16442e59d40670faf5ebb142e4); [probe: other Base contract lookup](https://api.coingecko.com/api/v3/coins/base/contract/0xa187403adcc6fcdf64fe9c850e0d9faf4ba3837f).
- Выполнено пять GET к `api.coingecko.com/api/v3`: asset platform directory один раз, BRETT/Zora address lookup сначала один раз с неверно закодированными необязательными query separators, затем повтор этих двух lookup с корректным query string. Все без API key/account, не чаще одного раза в 7 секунд, timeout 15s, response read capped at 2 MiB. Код проекта, runtime, DB и платные/аутентифицированные запросы не использовались.
