# Solana и Robinhood: direct ATH и free-plan кандидаты — 2026-10-11

## Вывод

Нашлись два direct ATH кандидата, но их бесплатность и исторический scope различаются:

- **Solana Tracker** документирует direct ATH endpoint и конкретный Free Data API plan: €0, 10,000 REST запросов/месяц, 3 req/s. Endpoint требует API key. Документы определяют ATH как «since the data API started recording», поэтому результат может быть максимумом только с момента, когда Solana Tracker начал собирать этот токен. Точный retention start per token, pool selection и Free-plan доступ к этому endpoint не установлены.
- **OpenDEX** документирует direct ATH price + market cap endpoint для `SOL` и `RH`, exact decimal strings, время, `since` и token/pair resolution с миграцией пары. Но хотя API reference помечает этот route `public` без Authorization header, два unauthenticated запроса к описанному production host получили HTTP 401. Отдельная first-party страница требует plan-backed `odx_b2b_` ключ для market data; указывает HTTP 402 при отсутствии активного paid/free subscription. Подтверждения доступного Free plan/quota и прямого пользовательского доступа нет, поэтому сейчас нельзя считать это free source.

Оба endpoint-returned ATH и market cap потенциально полезны как отдельные признаки для соответствующих сетей. Их нельзя приравнивать к историческому Codex `athFdv` без проверки denominator/supply method. Для полноты заполнять неудачный/неизвестный baseline нулем нельзя: у OpenDEX прямо описан fallback с `athMultiplier=1` и `athTimestamp=window start`, если собственно ATH результат отсутствует.

## Solana Tracker

Официальная документация [Get All-Time High Price](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price) раскрывается в текущем API index как:

```text
GET https://data.solanatracker.io/tokens/{tokenAddress}/ath
x-api-key: required
```

Endpoint summary называет метрику all-time high price, «since the data API started recording». В OpenAPI example показаны `highest_price`, `highest_market_cap`, `timestamp` и `pool_id`; schema перечисляет `highest_price` и `timestamp`, а example содержит market cap/pool дополнительно. SDK использует `client.getAthPrice(tokenAddress)`. Отдельного `since` параметра в endpoint contract не задокументировано, то есть вызывающий не может явно запросить ATH с launch-date или проверить baseline через этот route.

Официальная [Pricing & Limits](https://docs.solanatracker.io/pricing) сейчас указывает Free Data API: €0, 10,000 requests/month и 3 requests/sec. Тариф относится к Data API в целом. Pricing/docs не показывают endpoint-by-endpoint Free entitlement, поэтому доступность именно `/ath` в Free не подтверждена, хоть план и предоставляет общий REST доступ. Для ключа нужно использовать account/dashboard; live запросы не выполнялись, поскольку ключа нет и endpoint в документации требует ключ.

Поставщик отдельно заявляет широкую Solana DEX coverage на продуктовой [Data API page](https://www.solanatracker.io/data-api), но это не доказывает coverage конкретного токена или всех его pools. Поле `pool_id` в примере ATH указывает как минимум на привязку к конкретному pool result; API docs не объясняют multi-pool aggregation, migration semantics, supply basis для `highest_market_cap` или историю до provider tracking start. В исследованном контракте нет количественной retention guarantee.

Классификация: `docs` — direct ATH schema, required key, Free plan quota, historical scope starts at API recording; `unknown` — Free entitlement for endpoint, start date per token, all-pool coverage, migration/launch completeness and market-cap/FDV equivalence; `not live tested` — no authenticated request.

## OpenDEX

Официальная [ATH API reference](https://portal.opendex.ws/api-reference/token_ath) задаёт:

```text
GET /v2/token/{chain}/{address}/ath
chain ∈ {SOL, RH}
since = Unix timestamp in seconds (optional)
```

Документы описывают `athPriceUsd` / `athPriceNative` и `athMarketCapUsd` / `athMarketCapNative`, а также соответствующие exact decimal `...Str`, timestamp в milliseconds/ISO, `sinceTimestamp` и `athMultiplier`. При omitted `since` окно начинается с creation time разрешённого token или pair. Token address разрешается в primary pair; pair address остаётся pair-scoped и следует миграции в live pool. Это полезнее простой current-price/MCap точки, но `primary pair`/`live pool` формулировка не гарантирует агрегирование всех одновременно существующих рынков.

Документация противоречит сама себе о timestamp units: endpoint overview/parameter говорит `since` — Unix seconds, однако нижний раздел “Timestamp fields” утверждает, что `since` — milliseconds. До письменного уточнения/проверки нельзя строить запросы на `since`; omit `since` избегает этой конкретной неоднозначности.

Существенный sentinel: при отсутствии ATH результата endpoint возвращает baseline, multiplier 1 и `athTimestamp` равный window start. Поэтому успешный HTTP 200 не доказывает, что найдено фактическое экстремальное наблюдение. Клиенту нужно интерпретировать `athMultiplier == 1` вместе с timestamps как ambiguous/baseline, а не как доказанный ATH.

Документация помечает ATH route `public: no Authorization header is needed`, но официальный [Platform API access guide](https://portal.opendex.ws/docs/portal/api-access) говорит, что market-data API на `api.opendex.ws` использует plan-backed `odx_b2b_` `X-API-Key`; 401 — отсутствующий/невалидный ключ, 402 — нет активной paid или free subscription. Два живых GET без авторизации — по SOL примеру из API docs и по RH адресу `0x75e7a6aec1104da979adeb3757f892e430e37c60`, указанному родительским исследованием как известный индексированный RH token, — оба вернули HTTP 401. Response bodies не сохранялись. Это подтверждает, что docs label “public” означает отсутствие JWT Authorization для операции, но не доказывает keyless/free market-data access.

Официальная публичная документация не сообщает цену или квоту plan, не устанавливает, доступен ли бесплатный план для новой организации, и не указывает максимальный history depth. Поэтому цена/бесплатность/retention — `unknown`; данные API key требуют account/org setup, живой ответ не получен. Маршрут `RH` совпадает по имени с Robinhood Chain, но в просмотренном route contract не указан chain ID 4663; привязка именно к mainnet против возможной другой RH среды не установлена этим endpoint page.

Классификация: `docs` — direct ATH/MCap, exact strings, token/pair addressing, token creation default, automatic pair migration language, SOL/RH codes, baseline sentinel; `probe` — unauthenticated call returns 401 on production host; `unknown` — paid/free plan/quota, retention, RH network ID binding, source completeness and `athMarketCap` denominator semantics.

## Роль в общей схеме и короткий PoC

Потенциальная композиция для пользователя, если endpoint plans подтвердятся: сохранить Codex ATH как исторический provider anchor, оценить Solana Tracker для части Solana missing tokens/new observations, а OpenDEX отдельно оценивать для Solana + Robinhood только после выяснения free subscription/key entitlement и сравнения результата на нескольких известных токенах. Условный current/provider-tracking ATH нельзя подменять full-lifetime ATH; поля market cap не конвертировать в Codex FDV до подтверждения supply/time semantics.

Bounded следующий шаг (без ключа сейчас не выполнен): запросить владельца проекта/провайдера о точном Free-plan entitlement для OpenDEX ATH и Solana Tracker ATH, а также о tracking start/multipool/migration model. Если уже разрешённые credentials доступны локально, не показывать их и делать максимум 2 authenticated read-only tests per provider: один давно индексируемый token и один новый token. Проверить `ATH` against known chart `max(high)`, creation time, multiple pools, migration; фиксировать provider start bound и null/baseline отдельно. Этот документ не даёт оснований заводить аккаунт, покупать тариф или писать интеграцию.

## Действия и ограничения

- Для OpenDEX выполнены ровно два ограниченных keyless GET к official documented production host; оба HTTP 401. Запросы без ключей, без аккаунта, read-only; не повторялись.
- Solana Tracker docs endpoint проверен через официальный `llms.txt`, Markdown page и pricing file. API call не выполнялся: contract требует `x-api-key`, Free key/account не создавался.
- Не запускались project runtime, DB/procedures, Codex paid requests или mutations.
